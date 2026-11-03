import { Acks } from "./acks.ts";
import { Emitter } from "./emitter.ts";
import { Mixer, type MixerHost } from "./mixer.ts";
import {
  NativeEngine,
  type NativeEvent,
  type NativeMixer,
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
import { framesToMs, msToFrames } from "./units.ts";
import { finiteIn, ingestKind, integerIn, pcm } from "./validate.ts";

type State = "running" | "faulted" | "closed";

/** An engine on the test clock. Its caller pumps it; nothing plays. */
export type OfflineEngine = Engine & Required<Pick<Engine, "pump">>;

/**
 * Opens the engine for the life of the process: the device, or the offline driver, running the
 * silent graph until a mixer starts.
 */
export function openEngine(
  options: OpenEngineOptions & { output: { offline: true } },
): Promise<OfflineEngine>;
export function openEngine(options: OpenEngineOptions): Promise<Engine>;
export async function openEngine(options: OpenEngineOptions): Promise<Engine> {
  return new Engine(options);
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
  readonly output: OutputInfo;
  /** How many slots a mixer may have, fixed at open. */
  readonly #slots: number;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  readonly pump?: (blocks: number) => Float32Array;
  readonly #native: NativeEngine;
  readonly #host: MixerHost;
  readonly #acks = new Acks();
  readonly #events = new Emitter<EngineEvents>();
  /** Swaps in flight, oldest first; a `retired` event completes the oldest. */
  readonly #retirements: Array<() => void> = [];
  /** Render jobs on the worker thread, by the id their progress events carry. */
  readonly #renders = new Map<number, RenderWatch>();
  #live: { mixer: Mixer; events: Emitter<MixerEvents> } | null = null;
  #underruns = 0;
  #state: State = "running";

  constructor(options: OpenEngineOptions) {
    this.#slots = integerIn("slots", options.slots ?? 8, 1, 16);
    // The jitter buffer holds 64 packets ahead of its cursor, 160 ms of the phone's shortest
    // frame; under 5 ms, two blocks, that frame has no slack and ordinary jitter is all late packets
    const playoutDelayFrames = msToFrames(
      finiteIn("playoutDelayMs", options.playoutDelayMs ?? 15, 5, 100),
    );
    const deliver = (batch: NativeEvent[]) => {
      this.#deliver(batch);
    };
    if ("offline" in options.output) {
      this.#native = new NativeEngine(
        { offline: true, periodFrames: 128, playoutDelayFrames },
        deliver,
      );
      this.pump = (blocks) => this.#native.pump(integerIn("blocks", blocks, 1, 1 << 20));
    } else {
      const { device, periodFrames = 128 } = options.output;
      if (periodFrames !== 64 && periodFrames !== 128) {
        throw new RangeError(`periodFrames must be 64 or 128, got ${String(periodFrames)}`);
      }
      this.#native = new NativeEngine(
        {
          offline: false,
          periodFrames,
          playoutDelayFrames,
          ...(device === undefined ? {} : { device }),
        },
        deliver,
      );
    }
    this.output = outputInfo(this.#native.output);
    this.#host = {
      native: this.#native,
      output: this.output,
      command: (send) => this.#command(send),
      watchRender: (id, onProgress) =>
        new Promise<void>((resolve, reject) => {
          this.#renders.set(id, { onProgress, resolve, reject });
        }),
      swapIn: (mixer, graph, events) => this.#swapIn(mixer, graph, events),
      swapOut: (mixer) => this.#swapOut(mixer),
      isLive: (mixer) => this.#live?.mixer === mixer,
      anyLive: () => this.#live !== null,
      underruns: () => this.#underruns,
    };
  }

  /** Room-lifetime reaction voices, four at once; a fifth is dropped while four play. */
  readonly reactions = {
    play: (samples: Float32Array, channels: 1 | 2) => {
      if (channels !== 1 && channels !== 2) {
        throw new RangeError(`channels must be 1 or 2, got ${String(channels)}`);
      }
      pcm("samples", samples, channels);
      this.#assertRunning();
      this.#native.playReaction(channels === 2 ? samples : monoToStereo(samples));
    },
  };

  async outputDevices(): Promise<ReadonlyArray<{ id: string; name: string; isDefault: boolean }>> {
    this.#assertRunning();
    return this.#native.outputs();
  }

  async createMixer(config: MixerConfig): Promise<Mixer> {
    this.#assertRunning();
    if (config.slots.length > this.#slots) {
      throw new RangeError(
        `the engine has ${this.#slots} slots; the mixer asks for ${config.slots.length}`,
      );
    }
    const kinds = config.slots.map(({ kind }, i) => ingestKind(`slots[${i}].kind`, kind));
    if (config.rtpReturn) throw new Error("the return branch arrives with its workstream");
    return new Mixer(this.#host, kinds, new Emitter<MixerEvents>(), config.backing ?? null);
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
    this.#stop(new Error("the engine is closed"));
  }

  #swapIn(mixer: Mixer, graph: NativeMixer, events: Emitter<MixerEvents>) {
    if (this.#live) throw new Error("another mixer is live; destroy it first");
    const swapped = this.#command(() => this.#native.swap(graph));
    this.#live = { mixer, events };
    // The silent graph comes back; nothing waits for it
    this.#retirements.push(() => {});
    return swapped;
  }

  async #swapOut(mixer: Mixer) {
    if (this.#live?.mixer !== mixer) return;
    const swapped = this.#command(() => this.#native.swapSilent());
    this.#live = null;
    const retired = new Promise<void>((resolve) => this.#retirements.push(resolve));
    await swapped;
    await retired;
  }

  #command(send: () => number) {
    this.#assertRunning();
    return this.#acks.wait(send(), () => this.#state);
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
          );
          break;
        case "position":
          this.#live?.events.emit("position", framesToMs(event.frames));
          break;
        case "backingEnded":
          this.#live?.events.emit("backingEnded");
          break;
        case "masterLevel":
          break;
        case "underrun":
          this.#underruns += 1;
          break;
        case "fault":
          this.#fault();
          break;
        case "retired":
          this.#retirements.shift()?.();
          break;
        case "renderProgress":
          this.#renders.get(event.job)?.onProgress(event.fraction);
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

  /** A device loss or a caught panic: the host closes and reopens; nothing here recovers. */
  #fault() {
    if (this.#state !== "running") return;
    this.#state = "faulted";
    const error = new Error("the engine has faulted; close it and open it again");
    this.#stop(error);
    this.#events.emit("fault", error);
  }

  /** Nothing in flight will be answered: the commands and the renders waited on reject. */
  #stop(error: Error) {
    this.#acks.rejectAll(error);
    for (const render of this.#renders.values()) render.reject(error);
    this.#renders.clear();
  }
}

interface RenderWatch {
  onProgress(fraction: number): void;
  resolve(): void;
  reject(error: Error): void;
}

function outputInfo(info: NativeOutputInfo): OutputInfo {
  if (info.sampleRate !== 48000) {
    throw new Error(`the engine runs at 48 kHz; the addon reports ${info.sampleRate}`);
  }
  return { sampleRate: 48000, periodFrames: info.periodFrames, latencyMs: info.latencyMs };
}

function monoToStereo(mono: Float32Array) {
  const stereo = new Float32Array(mono.length * 2);
  mono.forEach((sample, i) => {
    stereo[2 * i] = sample;
    stereo[2 * i + 1] = sample;
  });
  return stereo;
}
