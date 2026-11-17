import { Acks } from "./acks.ts";
import { bundleRequest } from "./bundle/bundle-request.ts";
import { Emitter } from "./emitter.ts";
import { Mixer, type MixerHost } from "./mixer.ts";
import {
  NativeEngine,
  type NativeEvent,
  type NativeMixer,
  type NativeOpenOptions,
  type NativeOutputInfo,
} from "./native.ts";
import type {
  Capability,
  EngineEvents,
  MixerConfig,
  MixerEvents,
  OpenEngineOptions,
  OutputInfo,
} from "./types.ts";
import { OutputUnavailableError } from "./output-unavailable-error.ts";
import { defaultProfile } from "./profile/default-profile.ts";
import { deepFreeze } from "./profile/deep-freeze.ts";
import { parseProfile } from "./profile/parse-profile.ts";
import type { SettingsProfile } from "./profile/settings-profile.ts";
import { framesToMs, msToFrames } from "./units.ts";
import {
  finiteIn,
  integerIn,
  pcm,
  outputPeriod,
  RANGES,
  rtpReturn,
  slotInput,
} from "./validate.ts";

type State = "running" | "faulted" | "closed";

/** Carries the addon's event batches to the engine `openEngine` builds once the open resolves. */
type Batches = Emitter<{ batch: (batch: NativeEvent[]) => void }>;

/** An engine on the test clock. Its caller pumps it; nothing plays. */
export type OfflineEngine = Engine & Required<Pick<Engine, "pump">>;

/**
 * Opens the engine for the life of the process: the device, the timer clock on a box with no
 * sound card, or the offline driver, running the silent graph until a mixer starts.
 */
export function openEngine(
  options: OpenEngineOptions & { output: { offline: true } },
): Promise<OfflineEngine>;
export function openEngine(options: OpenEngineOptions): Promise<Engine>;
export async function openEngine(options: OpenEngineOptions): Promise<Engine> {
  const slots = integerIn("slots", options.slots ?? 8, 1, 16);
  const profile = resolveProfile(options);
  const batches: Batches = new Emitter();
  // The events thread starts as the open resolves and its first batch lands on a later turn of the
  // event loop, after the engine below listens
  const deliver = (batch: NativeEvent[]) => batches.emit("batch", batch);
  try {
    const native = await NativeEngine.open(nativeOptions(options.output, profile), deliver);
    return new Engine(native, slots, profile, "offline" in options.output, batches);
  } catch (error) {
    throw OutputUnavailableError.from(error);
  }
}

/**
 * The profile the engine runs under: `options.profile`, checked and copied so the host's object
 * can change without it, with `output.periodFrames` and `playoutDelayMs` folded in where given.
 */
function resolveProfile(options: OpenEngineOptions): SettingsProfile {
  const profile = parseProfile(options.profile ?? defaultProfile);
  const period = "periodFrames" in options.output ? options.output.periodFrames : undefined;
  if (period !== undefined) {
    profile.output.periodFrames = outputPeriod("periodFrames", period);
  }
  if (options.playoutDelayMs !== undefined) {
    profile.ingest.playoutDelayMs = finiteIn(
      "playoutDelayMs",
      options.playoutDelayMs,
      ...RANGES.playoutDelayMs,
    );
  }
  return deepFreeze(profile);
}

/** The addon's options for `output` under `profile`. The timer and offline clocks have no period of their own. */
function nativeOptions(
  output: OpenEngineOptions["output"],
  { output: { periodFrames }, ingest: { playoutDelayMs } }: SettingsProfile,
): NativeOpenOptions {
  const pinned =
    playoutDelayMs === "auto" ? {} : { playoutDelayFrames: msToFrames(playoutDelayMs) };
  if ("offline" in output) return { clock: "offline", periodFrames: 128, ...pinned };
  if ("timer" in output) return { clock: "timer", periodFrames: 128, ...pinned };
  const { device } = output;
  return { clock: "device", periodFrames, ...pinned, ...(device === undefined ? {} : { device }) };
}

export class Engine {
  readonly capabilities: ReadonlySet<Capability> = new Set<Capability>([
    "guideVocal",
    "key",
    "speed",
    "seek",
    "shortVersion",
    "pause",
    "level",
  ]);
  /** The profile in force, with `periodFrames` and `playoutDelayMs` folded in where they were given. */
  readonly profile: SettingsProfile;
  #output: OutputInfo;
  /** How many slots a mixer may have, fixed at open. */
  readonly #slots: number;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  readonly pump?: (blocks: number) => Float32Array;
  readonly #native: NativeEngine;
  readonly #host: MixerHost;
  readonly #acks = new Acks();
  readonly #events = new Emitter<EngineEvents>();
  /** Swaps in flight, oldest first; a `retired` event resolves the oldest. */
  readonly #retirements: Waiter[] = [];
  /** Each mixer's release until it is destroyed; `close` calls whatever is left. */
  readonly #held = new Set<() => void>();
  /** Render jobs on the worker thread, by the id their progress events carry. */
  readonly #renders = new Map<number, RenderWatch>();
  #live: { mixer: Mixer; events: Emitter<MixerEvents> } | null = null;
  #underruns = 0;
  #state: State = "running";

  /** Built by `openEngine` around the engine the addon opened, whose events `batches` relays. */
  constructor(
    native: NativeEngine,
    slots: number,
    profile: SettingsProfile,
    offline: boolean,
    batches: Batches,
  ) {
    this.#native = native;
    this.#slots = slots;
    this.profile = profile;
    batches.on("batch", (batch) => {
      this.#deliver(batch);
    });
    if (offline) {
      this.pump = (blocks) => this.#native.pump(integerIn("blocks", blocks, 1, 1 << 20));
    }
    this.#output = outputInfo(this.#native.output);
    this.#host = {
      native: this.#native,
      profile,
      output: () => this.#output,
      command: (send, onTimeout) => this.#command(send, onTimeout),
      watchRender: (id, onProgress) =>
        new Promise<void>((resolve, reject) => {
          this.#renders.set(id, { onProgress, resolve, reject });
        }),
      swapIn: (mixer, graph, events, onTimeout) => this.#swapIn(mixer, graph, events, onTimeout),
      swapOut: (mixer) => this.#swapOut(mixer),
      onFault: (listener) => this.#events.on("fault", listener),
      onOutputChanged: (listener) => this.#events.on("outputChanged", listener),
      hold: (release) => {
        this.#held.add(release);
        return () => this.#held.delete(release);
      },
      isLive: (mixer) => this.#live?.mixer === mixer,
      anyLive: () => this.#live !== null,
      underruns: () => this.#underruns,
    };
  }

  /**
   * The output playing now. An engine opened with no `output.device` follows the OS default, and
   * one with a `device` plays there while it exists; `outputChanged` fires after every move, and
   * when `refused` clears with no move.
   */
  get output() {
    return this.#output;
  }

  /** Room-lifetime reaction voices, four at once. False when a fifth is dropped while four play. */
  readonly reactions = {
    play: (samples: Float32Array, channels: 1 | 2) => {
      if (channels !== 1 && channels !== 2) {
        throw new RangeError(`channels must be 1 or 2, got ${String(channels)}`);
      }
      pcm("samples", samples, channels);
      this.#assertRunning();
      return this.#native.playReaction(channels === 2 ? samples : monoToStereo(samples));
    },
  };

  /**
   * The outputs this engine may move to: on a device engine, what the module's `outputDevices()`
   * lists. None on the timer and offline clocks, which play on no output; the module's call lists
   * the OS's whatever engine is open.
   */
  async outputDevices(): Promise<ReadonlyArray<{ id: string; name: string; isDefault: boolean }>> {
    this.#assertRunning();
    return this.#native.outputs();
  }

  /**
   * The inputs a `capture` slot may name, the default marked: the module's `inputDevices()`. None
   * on an offline engine, which opens no input.
   */
  async inputDevices(): Promise<ReadonlyArray<{ id: string; name: string; isDefault: boolean }>> {
    this.#assertRunning();
    return this.#native.inputs();
  }

  async createMixer(config: MixerConfig): Promise<Mixer> {
    this.#assertRunning();
    if (config.slots.length > this.#slots) {
      throw new RangeError(
        `the engine has ${this.#slots} slots; the mixer asks for ${config.slots.length}`,
      );
    }
    const slots = config.slots.map((slot, i) => slotInput(`slots[${i}]`, slot));
    const destination = config.rtpReturn === undefined ? null : rtpReturn(config.rtpReturn);
    const bundle =
      config.bundle === undefined
        ? null
        : await bundleRequest(config.bundle, config.backing, this.profile.backing.stemGains);
    // The engine can fault or close while the stems hash, and a mixer is built on a running one
    this.#assertRunning();
    return new Mixer(
      this.#host,
      slots,
      new Emitter<MixerEvents>(),
      config.backing ?? null,
      destination,
      bundle,
    );
  }

  on<K extends keyof EngineEvents>(event: K, listener: EngineEvents[K]) {
    return this.#events.on(event, listener);
  }

  /** Destroys the live mixer, stops the clock and joins the events thread. */
  async close() {
    if (this.#state === "closed") return;
    if (this.#live && this.#state === "running") {
      await this.#live.mixer.destroy();
    }
    this.#state = "closed";
    this.#native.close();
    // Armed mixers, and a live one on a faulted engine, still hold their inputs and ports
    for (const release of this.#held) release();
    this.#stop(new Error("the engine is closed"));
  }

  #swapIn(
    mixer: Mixer,
    graph: NativeMixer,
    events: Emitter<MixerEvents>,
    onTimeout: (error: Error) => void,
  ) {
    if (this.#live) throw new Error("another mixer is live; destroy it first");
    const swapped = this.#command(() => this.#native.swap(graph), onTimeout);
    this.#live = { mixer, events };
    // The silent graph comes back; nothing waits for it
    this.#retirements.push({ resolve() {}, reject() {} });
    return swapped;
  }

  async #swapOut(mixer: Mixer) {
    if (this.#live?.mixer !== mixer) return;
    // A stopped engine takes no commands and hands nothing back; its graph goes with the clock
    if (this.#state !== "running") {
      this.#live = null;
      return;
    }
    const swapped = this.#command(() => this.#native.swapSilent());
    this.#live = null;
    const retired = new Promise<void>((resolve, reject) => {
      this.#retirements.push({ resolve, reject });
    });
    // A stopping engine rejects both, and both are awaited so neither rejection goes unheard
    await Promise.all([swapped, retired]);
  }

  #command(send: () => number, onTimeout?: (error: Error) => void) {
    this.#assertRunning();
    return this.#acks.wait(send(), () => this.#state, onTimeout);
  }

  #assertRunning() {
    if (this.#state !== "running") {
      throw new Error(
        this.#state === "faulted"
          ? "the engine has faulted; close it and open it again"
          : "the engine is closed",
      );
    }
  }

  #deliver(batch: NativeEvent[]) {
    if (this.#state === "closed") return;
    for (const event of batch) {
      switch (event.kind) {
        case "ack":
          this.#acks.resolve(event.id, event.positionFrames);
          break;
        case "level":
          this.#live?.events.emit("level", event.slot, event.rmsDb, event.peakDb);
          break;
        case "pitch":
          this.#live?.events.emit(
            "pitch",
            event.slot,
            event.positionMs,
            event.hz,
            event.confidence,
            event.targetMidi ?? null,
            event.errorSemitones,
          );
          break;
        case "position":
          this.#live?.events.emit("position", framesToMs(event.frames));
          break;
        case "backingEnded":
          this.#live?.events.emit("backingEnded");
          break;
        case "masterLevel":
          this.#events.emit("masterLevel", event.rmsDb, event.peakDb);
          break;
        case "underrun":
          this.#underruns += 1;
          break;
        case "fault":
          this.#fault();
          break;
        case "retired":
          this.#retirements.shift()?.resolve();
          break;
        case "renderProgress":
          this.#renders.get(event.job)?.onProgress(event.fraction);
          break;
        case "outputChanged":
          this.#output = outputInfo(event.output);
          this.#events.emit("outputChanged", this.#output, event.gapMs);
          break;
        case "renderFinished": {
          const render = this.#renders.get(event.job);
          this.#renders.delete(event.job);
          render?.resolve();
          break;
        }
      }
    }
  }

  /**
   * A device loss or a caught panic: the host closes and reopens; nothing here recovers. `fault`
   * reaches the host's listeners and every mixer not yet destroyed, each of which emits `error`.
   */
  #fault() {
    if (this.#state !== "running") return;
    this.#state = "faulted";
    const error = new Error("the engine has faulted; close it and open it again");
    this.#stop(error);
    this.#events.emit("fault", error);
  }

  /**
   * Nothing in flight will be answered: every promise waiting on the audio thread rejects. A new
   * wait on an event joins this list.
   */
  #stop(error: Error) {
    this.#acks.rejectAll(error);
    for (const waiter of [...this.#renders.values(), ...this.#retirements]) waiter.reject(error);
    this.#renders.clear();
    this.#retirements.length = 0;
  }
}

/** A promise's settlers, kept until the event it waits on arrives or the engine stops. */
interface Waiter {
  resolve(): void;
  reject(error: Error): void;
}

interface RenderWatch extends Waiter {
  onProgress(fraction: number): void;
}

function outputInfo(info: NativeOutputInfo): OutputInfo {
  if (info.sampleRate !== 48000) {
    throw new Error(`the engine runs at 48 kHz; the addon reports ${info.sampleRate}`);
  }
  const { periodFrames, latencyMs, device, refused } = info;
  return {
    sampleRate: 48000,
    periodFrames,
    latencyMs,
    device: device === undefined ? null : { id: device.id, name: device.name },
    refused:
      refused === undefined ? null : { id: refused.id, name: refused.name, error: refused.error },
  };
}

function monoToStereo(mono: Float32Array) {
  const stereo = new Float32Array(mono.length * 2);
  mono.forEach((sample, i) => {
    stereo[2 * i] = sample;
    stereo[2 * i + 1] = sample;
  });
  return stereo;
}
