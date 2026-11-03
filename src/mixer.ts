import { Backing, type BackingConfig, type RenderHost, STOP_FADE_MS } from "./backing.ts";
import type { Emitter } from "./emitter.ts";
import type { GuideInput, SlotParamsInput } from "./native.ts";
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
import { framesToMs, msToFrames } from "./units.ts";
import { finiteIn, integerIn, pcm } from "./validate.ts";

/** What a mixer needs from its engine. Internal to the package. */
export interface MixerHost extends RenderHost {
  readonly output: OutputInfo;
  /** Sends a command carrying an id and resolves on its ack with the backing position. */
  command(send: () => number): Promise<number>;
  /** Swaps `graph` in as the live graph, routing its events to `events`. Rejects while another mixer is live. */
  swapIn(mixer: Mixer, graph: NativeMixer, events: Emitter<MixerEvents>): Promise<number>;
  /** Swaps the silent graph back in; resolves once the mixer's graph is back and dropped. */
  swapOut(mixer: Mixer): Promise<void>;
  isLive(mixer: Mixer): boolean;
  anyLive(): boolean;
  underruns(): number;
}

type Phase = "armed" | "starting" | "live" | "destroyed";

/** The ingest stats a push slot reports: only its resyncs mean anything until ingest lands. */
function pushSlotStats(resyncs: number): SlotStats {
  return { pushed: 0, lost: 0, late: 0, concealed: 0, resyncs, jitterMs: 0 };
}

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
  readonly #backing: Backing | null;
  #phase: Phase = "armed";

  constructor(
    host: MixerHost,
    kinds: ReadonlyArray<IngestKind>,
    events: Emitter<MixerEvents>,
    backing: BackingConfig | null,
  ) {
    this.#host = host;
    this.#events = events;
    this.#native = new NativeMixer(kinds.length);
    this.slots = kinds.map((kind, index) => ({ index, kind, port: null }));
    this.#params = kinds.map(defaultSlotParams);
    this.#guides = kinds.map(() => null);
    this.#backing =
      backing === null
        ? null
        : new Backing(host, backing, (fraction) => {
            if (this.#phase !== "destroyed") this.#events.emit("renderProgress", fraction);
          });
  }

  setSlot(index: number, settings: Partial<SlotSettings>) {
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
  setSlotEffects(index: number, effects: ReadonlyArray<SlotEffect>) {
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

  setSlotGuide(index: number, guide: MelodyGuide | null, guideOffsetMs?: number) {
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
  resetSlot(index: number) {
    const slot = this.#slot(index);
    if (this.#host.isLive(this)) {
      this.#host.native.resetSlot(slot);
    }
  }

  /**
   * Hands interleaved stereo PCM to a push slot, folded to mono as every slot is. Returns the
   * frames taken; the rest did not fit and is dropped, since late audio is worse than lost audio.
   */
  push(index: number, stereo: Float32Array) {
    const slot = this.#slot(index);
    if (this.#phase === "destroyed") {
      throw new Error("the mixer is destroyed");
    }
    return this.#native.push(slot, pcm("stereo", stereo, 2));
  }

  /**
   * Waits for the backing's render, installs it, and swaps the graph in. With the render already
   * done, the swap is sent before this returns to the caller. Nothing is written to the engine
   * while another mixer is live, so a refused start leaves the song playing untouched.
   */
  async start() {
    if (this.#phase !== "armed") {
      throw new Error(`the mixer cannot start: it is ${this.#phase}`);
    }
    this.#phase = "starting";
    let swapped: Promise<number>;
    try {
      if (this.#backing?.rendering) await this.#backing.rendered;
      if (this.#host.anyLive()) throw new Error("another mixer is live; destroy it first");
      this.#backing?.install(this.#native);
      this.#params.forEach((params, slot) => {
        this.#host.native.setSlotParams(slot, params);
      });
      if (this.#backing) this.#host.native.setStemGains(this.#backing.stemGains);
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
    const stopAtFrames = this.#backing?.stopAtFrames;
    const stopping =
      stopAtFrames === undefined
        ? undefined
        : this.#host.command(() =>
            this.#host.native.stopAt(stopAtFrames, msToFrames(STOP_FADE_MS)),
          );
    await Promise.all([swapped, started, stopping]);
    const { latencyMs } = this.#host.output;
    return {
      startedAt: Date.now() + latencyMs,
      backingOffsetMs: latencyMs,
      speedFactor: this.#backing?.speed ?? 1,
    };
  }

  /** Resolves with the position the seek landed on, once its fade out is done. */
  async seek(toMs: number) {
    const toFrame = msToFrames(finiteIn("toMs", toMs, 0, Number.MAX_SAFE_INTEGER));
    const native = this.#liveBacking();
    const landed = await this.#host.command(() => native.seek(toFrame));
    return { positionMs: framesToMs(landed) };
  }

  /** Resolves once the fade out is done and the playhead holds. */
  async pause() {
    const native = this.#liveBacking();
    await this.#host.command(() => native.pause());
  }

  async resume() {
    const native = this.#liveBacking();
    const position = await this.#host.command(() => native.resume());
    return { positionMs: framesToMs(position) };
  }

  /** The live key, on top of the armed one; its crossfade follows the ack. */
  async setBackingKey(semitones: number) {
    const checked = integerIn("semitones", semitones, -12, 12);
    const native = this.#liveBacking();
    await this.#host.command(() => native.setKey(checked));
  }

  /** A parameter write, held while armed like the slot settings. */
  setStemGain(kind: StemKind, gain: number) {
    if (!this.#backing) throw new Error("this mixer has no backing");
    const gains = this.#backing.setStemGain(kind, gain);
    if (this.#host.isLive(this)) {
      this.#host.native.setStemGains(gains);
    }
  }

  on<K extends keyof MixerEvents>(event: K, listener: MixerEvents[K]) {
    return this.#events.on(event, listener);
  }

  stats(): { slots: ReadonlyArray<SlotStats>; underruns: number } {
    return {
      slots: this.slots.map((slot) => pushSlotStats(this.#native.resyncs(slot.index))),
      underruns: this.#host.underruns(),
    };
  }

  /** Swaps the silent graph in and resolves once this graph is back and dropped. */
  async destroy() {
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

  #slot(index: number) {
    return integerIn("slot", index, 0, this.slots.length - 1);
  }

  /** The engine, once this mixer's backing is the live one. */
  #liveBacking() {
    if (!this.#backing) throw new Error("this mixer has no backing");
    if (!this.#host.isLive(this)) {
      throw new Error(`the backing cannot be steered: the mixer is ${this.#phase}`);
    }
    return this.#host.native;
  }

  #paramsOf(slot: number) {
    const params = this.#params[slot];
    if (!params) throw new RangeError(`no slot ${slot}`);
    return params;
  }

  #write(slot: number, params: SlotParamsInput) {
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
