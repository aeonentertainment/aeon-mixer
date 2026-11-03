import type { Emitter } from "./emitter.ts";
import type { GuideInput, NativeEngine, SlotParamsInput } from "./native.ts";
import { NativeMixer } from "./native.ts";
import type {
  IngestKind,
  MelodyGuide,
  MixerEvents,
  OutputInfo,
  SlotEffect,
  SlotSettings,
  SlotStats,
  StemKind,
} from "./types.ts";
import { msToFrames } from "./units.ts";
import { finiteIn, integerIn, pcm } from "./validate.ts";

/** What a mixer needs from its engine. Internal to the package. */
export interface MixerHost {
  readonly native: NativeEngine;
  readonly output: OutputInfo;
  /** Sends a command carrying an id and resolves on its ack with the backing position. */
  command(send: () => number): Promise<number>;
  /** Swaps `graph` in as the live graph, routing its events to `events`. Rejects while another mixer is live. */
  swapIn(mixer: Mixer, graph: NativeMixer, events: Emitter<MixerEvents>): Promise<number>;
  /** Swaps the silent graph back in; resolves once the mixer's graph is back and dropped. */
  swapOut(mixer: Mixer): Promise<void>;
  isLive(mixer: Mixer): boolean;
  underruns(): number;
}

type Phase = "armed" | "starting" | "live" | "destroyed";

const NOT_YET = "the backing path arrives with the backing workstream";

/** The ingest stats a slot reports; all zero until the ingest workstream fills them. */
const NO_SLOT_STATS: SlotStats = {
  pushed: 0,
  lost: 0,
  late: 0,
  concealed: 0,
  resyncs: 0,
  jitterMs: 0,
};

function defaultSlotParams(): SlotParamsInput {
  return {
    gain: 1,
    pan: 0,
    mute: false,
    solo: false,
    corrector: { enabled: false, strength: 0, retuneMs: 0 },
    reverb: { enabled: false, roomSize: 0, mix: 0 },
    guideOffsetMs: 0,
  };
}

/**
 * One mixing graph for one performance. Settings made while armed are kept here and written to
 * the engine when `start` swaps the graph in, so arming the next song never touches the live one.
 */
export class Mixer {
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind; port: number | null }>;
  readonly #host: MixerHost;
  readonly #native: NativeMixer;
  readonly #events: Emitter<MixerEvents>;
  readonly #params: SlotParamsInput[];
  readonly #guides: Array<GuideInput | null>;
  #phase: Phase = "armed";

  constructor(host: MixerHost, kinds: ReadonlyArray<IngestKind>, events: Emitter<MixerEvents>) {
    this.#host = host;
    this.#events = events;
    this.#native = new NativeMixer(kinds.length);
    this.slots = kinds.map((kind, index) => ({ index, kind, port: null }));
    this.#params = kinds.map(defaultSlotParams);
    this.#guides = kinds.map(() => null);
  }

  setSlot(index: number, settings: Partial<SlotSettings>): void {
    const slot = this.#slot(index);
    const current = this.#paramsOf(slot);
    this.#write(slot, {
      ...current,
      gain: settings.gain === undefined ? current.gain : finiteIn("gain", settings.gain, 0, 4),
      pan: settings.pan === undefined ? current.pan : finiteIn("pan", settings.pan, -1, 1),
      mute: settings.mute ?? current.mute,
      solo: settings.solo ?? current.solo,
    });
  }

  /** The slot's whole chain: an effect not in `effects` is disabled. */
  setSlotEffects(index: number, effects: ReadonlyArray<SlotEffect>): void {
    const slot = this.#slot(index);
    const next = { ...this.#paramsOf(slot), ...defaultEffects() };
    for (const effect of effects) {
      switch (effect.kind) {
        case "pitch-correction":
          next.corrector = {
            enabled: effect.enabled,
            strength: finiteIn("strength", effect.strength, 0, 1),
            retuneMs: finiteIn("retuneMs", effect.retuneMs, 0, 1000),
          };
          break;
        case "reverb":
          next.reverb = {
            enabled: effect.enabled,
            roomSize: finiteIn("roomSize", effect.roomSize, 0, 1),
            mix: finiteIn("mix", effect.mix, 0, 1),
          };
          break;
      }
    }
    this.#write(slot, next);
  }

  setSlotGuide(index: number, guide: MelodyGuide | null, guideOffsetMs?: number): void {
    const slot = this.#slot(index);
    const input = guide === null ? null : toGuideInput(guide);
    if (guideOffsetMs !== undefined) {
      this.#write(slot, {
        ...this.#paramsOf(slot),
        guideOffsetMs: finiteIn("guideOffsetMs", guideOffsetMs, -1000, 1000),
      });
    }
    this.#guides[slot] = input;
    if (this.#host.isLive(this)) {
      this.#host.native.setGuide(slot, input);
    }
  }

  /** Forget the stream a slot was hearing; the next audio is a new singer. */
  resetSlot(index: number): void {
    const slot = this.#slot(index);
    if (this.#host.isLive(this)) {
      this.#host.native.resetSlot(slot);
    }
  }

  /** Hands mono PCM to a push slot. Returns the frames taken; the rest did not fit and is dropped. */
  push(index: number, samples: Float32Array): number {
    const slot = this.#slot(index);
    if (this.#phase === "destroyed") {
      throw new Error("the mixer is destroyed");
    }
    return this.#native.push(slot, pcm("samples", samples, 1));
  }

  async start(): Promise<{ startedAt: number; backingOffsetMs: number; speedFactor: number }> {
    if (this.#phase !== "armed") {
      throw new Error(`the mixer cannot start: it is ${this.#phase}`);
    }
    this.#phase = "starting";
    let swapped: Promise<number>;
    try {
      this.#params.forEach((params, slot) => {
        this.#host.native.setSlotParams(slot, params);
      });
      swapped = this.#host.swapIn(this, this.#native, this.#events);
    } catch (error) {
      this.#phase = "armed";
      throw error;
    }
    // From here the graph is the audio thread's, so the mixer is live whatever the acks say.
    this.#phase = "live";
    this.#guides.forEach((guide, slot) => {
      if (guide) this.#host.native.setGuide(slot, guide);
    });
    const started = this.#host.command(() => this.#host.native.start());
    await Promise.all([swapped, started]);
    const { latencyMs } = this.#host.output;
    return { startedAt: Date.now() + latencyMs, backingOffsetMs: latencyMs, speedFactor: 1 };
  }

  seek(toMs: number): Promise<{ positionMs: number }> {
    finiteIn("toMs", toMs, 0, Number.MAX_SAFE_INTEGER);
    return Promise.reject(new Error(NOT_YET));
  }

  pause(): Promise<void> {
    return Promise.reject(new Error(NOT_YET));
  }

  resume(): Promise<{ positionMs: number }> {
    return Promise.reject(new Error(NOT_YET));
  }

  setBackingKey(semitones: number): Promise<void> {
    integerIn("semitones", semitones, -12, 12);
    return Promise.reject(new Error(NOT_YET));
  }

  setStemGain(kind: StemKind, gain: number): void {
    finiteIn(`${kind} gain`, gain, 0, 4);
    throw new Error(NOT_YET);
  }

  on<K extends keyof MixerEvents>(event: K, listener: MixerEvents[K]): () => void {
    return this.#events.on(event, listener);
  }

  stats(): { slots: ReadonlyArray<SlotStats>; underruns: number } {
    return { slots: this.slots.map(() => NO_SLOT_STATS), underruns: this.#host.underruns() };
  }

  /** Swaps the silent graph in and resolves once this graph is back and dropped. */
  async destroy(): Promise<void> {
    switch (this.#phase) {
      case "destroyed":
        return;
      case "starting":
        throw new Error("the mixer is starting; await start before destroying it");
      case "armed":
        this.#phase = "destroyed";
        return;
      case "live":
        this.#phase = "destroyed";
        await this.#host.swapOut(this);
    }
  }

  #slot(index: number): number {
    return integerIn("slot", index, 0, this.slots.length - 1);
  }

  #paramsOf(slot: number): SlotParamsInput {
    const params = this.#params[slot];
    if (!params) throw new RangeError(`no slot ${slot}`);
    return params;
  }

  #write(slot: number, params: SlotParamsInput): void {
    this.#params[slot] = params;
    if (this.#host.isLive(this)) {
      this.#host.native.setSlotParams(slot, params);
    }
  }
}

function defaultEffects(): Pick<SlotParamsInput, "corrector" | "reverb"> {
  const { corrector, reverb } = defaultSlotParams();
  return { corrector, reverb };
}

function toGuideInput(guide: MelodyGuide): GuideInput {
  return {
    keyMask: integerIn("keyMask", guide.keyMask, 0, 0xfff),
    notes: guide.notes.map((note, i) => {
      const startMs = finiteIn(`notes[${i}].startMs`, note.startMs, 0, Number.MAX_SAFE_INTEGER);
      return {
        midi: integerIn(`notes[${i}].midi`, note.midi, 0, 127),
        startFrame: msToFrames(startMs),
        endFrame: msToFrames(
          finiteIn(`notes[${i}].endMs`, note.endMs, startMs, Number.MAX_SAFE_INTEGER),
        ),
      };
    }),
  };
}
