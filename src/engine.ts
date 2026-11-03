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
import { framesToMs } from "./units.ts";
import { finiteIn, integerIn, pcm } from "./validate.ts";

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
  /** What this build can do; the rest arrives with the backing workstream. */
  readonly capabilities: ReadonlySet<Capability> = new Set<Capability>(["level"]);
  readonly output: OutputInfo;
  /** How many slots a mixer may have, fixed at open. */
  readonly slots: number;
  /** The ingest jitter buffer's playout delay, applied when the ingest workstream lands. */
  readonly playoutDelayMs: number;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  readonly pump?: (blocks: number) => Float32Array;
  readonly #native: NativeEngine;
  readonly #host: MixerHost;
  readonly #acks = new Acks();
  readonly #events = new Emitter<EngineEvents>();
  /** Swaps in flight, oldest first; a `retired` event completes the oldest. */
  readonly #retirements: Array<() => void> = [];
  #live: { mixer: Mixer; events: Emitter<MixerEvents> } | null = null;
  #underruns = 0;
  #state: State = "running";

  constructor(options: OpenEngineOptions) {
    this.slots = integerIn("slots", options.slots ?? 8, 1, 16);
    this.playoutDelayMs = finiteIn("playoutDelayMs", options.playoutDelayMs ?? 15, 1, 500);
    const deliver = (batch: NativeEvent[]) => {
      this.#deliver(batch);
    };
    if ("offline" in options.output) {
      this.#native = new NativeEngine({ offline: true, periodFrames: 128 }, deliver);
      this.pump = (blocks) => this.#native.pump(integerIn("blocks", blocks, 1, 1 << 20));
    } else {
      const { device, periodFrames = 128 } = options.output;
      if (periodFrames !== 64 && periodFrames !== 128) {
        throw new RangeError(`periodFrames must be 64 or 128, got ${String(periodFrames)}`);
      }
      this.#native = new NativeEngine(
        { offline: false, periodFrames, ...(device === undefined ? {} : { device }) },
        deliver,
      );
    }
    this.output = outputInfo(this.#native.output);
    this.#host = {
      native: this.#native,
      output: this.output,
      command: (send) => this.#command(send),
      swapIn: (mixer, graph, events) => this.#swapIn(mixer, graph, events),
      swapOut: (mixer) => this.#swapOut(mixer),
      isLive: (mixer) => this.#live?.mixer === mixer,
      underruns: () => this.#underruns,
    };
  }

  /** Room-lifetime reaction voices, four at once; a fifth is dropped while four play. */
  readonly reactions = {
    play: (samples: Float32Array, channels: 1 | 2): void => {
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
    if (config.slots.length > this.slots) {
      throw new RangeError(
        `the engine has ${this.slots} slots; the mixer asks for ${config.slots.length}`,
      );
    }
    for (const { kind } of config.slots) {
      if (kind !== "push") {
        throw new Error(
          `${kind} slots arrive with the ingest workstream; only push slots exist yet`,
        );
      }
    }
    if (config.backing) throw new Error("the backing path arrives with the backing workstream");
    if (config.rtpReturn) throw new Error("the return branch arrives with its workstream");
    return new Mixer(
      this.#host,
      config.slots.map(({ kind }) => kind),
      new Emitter<MixerEvents>(),
    );
  }

  on<K extends keyof EngineEvents>(event: K, listener: EngineEvents[K]): () => void {
    return this.#events.on(event, listener);
  }

  /** Destroys the live mixer, stops the clock and joins the events thread. */
  async close(): Promise<void> {
    if (this.#state === "closed") return;
    if (this.#live && this.#state === "running") {
      await this.#live.mixer.destroy();
    }
    this.#state = "closed";
    this.#native.close();
    this.#acks.rejectAll(new Error("the engine is closed"));
  }

  #swapIn(mixer: Mixer, graph: NativeMixer, events: Emitter<MixerEvents>): Promise<number> {
    if (this.#live) throw new Error("another mixer is live; destroy it first");
    const swapped = this.#command(() => this.#native.swap(graph));
    this.#live = { mixer, events };
    // The silent graph comes back; nothing waits for it
    this.#retirements.push(() => {});
    return swapped;
  }

  async #swapOut(mixer: Mixer): Promise<void> {
    if (this.#live?.mixer !== mixer) return;
    const swapped = this.#command(() => this.#native.swapSilent());
    this.#live = null;
    const retired = new Promise<void>((resolve) => this.#retirements.push(resolve));
    await swapped;
    await retired;
  }

  #command(send: () => number): Promise<number> {
    this.#assertRunning();
    return this.#acks.wait(send(), () => this.#state);
  }

  #assertRunning(): void {
    if (this.#state !== "running") {
      throw new Error(
        this.#state === "faulted"
          ? "the engine has faulted; close it and open it again"
          : "the engine is closed",
      );
    }
  }

  #deliver(batch: NativeEvent[]): void {
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
      }
    }
  }

  /** A device loss or a caught panic: the host closes and reopens; nothing here recovers. */
  #fault(): void {
    if (this.#state !== "running") return;
    this.#state = "faulted";
    const error = new Error("the engine has faulted; close it and open it again");
    this.#acks.rejectAll(error);
    this.#events.emit("fault", error);
  }
}

function outputInfo(info: NativeOutputInfo): OutputInfo {
  if (info.sampleRate !== 48000) {
    throw new Error(`the engine runs at 48 kHz; the addon reports ${info.sampleRate}`);
  }
  return { sampleRate: 48000, periodFrames: info.periodFrames, latencyMs: info.latencyMs };
}

function monoToStereo(mono: Float32Array): Float32Array {
  const stereo = new Float32Array(mono.length * 2);
  mono.forEach((sample, i) => {
    stereo[2 * i] = sample;
    stereo[2 * i + 1] = sample;
  });
  return stereo;
}
