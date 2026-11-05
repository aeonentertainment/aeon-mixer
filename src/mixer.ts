import { Backing, type BackingConfig, type RenderHost, STOP_FADE_MS } from "./backing.ts";
import type { Emitter } from "./emitter.ts";
import type { GuideInput, NativeMixer, RtpReturnInput, SlotParamsInput } from "./native.ts";
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
  /**
   * Sends a command carrying an id and resolves on its ack with the backing position. `onTimeout`
   * hears the rejection when no ack comes in time.
   */
  command(send: () => number, onTimeout: (error: Error) => void): Promise<number>;
  /**
   * Swaps `graph` in as the live graph, routing its events to `events`. Rejects while another
   * mixer is live. `onTimeout` is `command`'s.
   */
  swapIn(
    mixer: Mixer,
    graph: NativeMixer,
    events: Emitter<MixerEvents>,
    onTimeout: (error: Error) => void,
  ): Promise<number>;
  /**
   * Swaps the silent graph back in; resolves once the mixer's graph is back and dropped, or at
   * once on a faulted engine, which hands nothing back.
   */
  swapOut(mixer: Mixer): Promise<void>;
  /** Hears the engine's fault until the returned function is called. */
  onFault(listener: (error: Error) => void): () => void;
  isLive(mixer: Mixer): boolean;
  anyLive(): boolean;
  underruns(): number;
}

type Phase = "armed" | "starting" | "live" | "destroyed";

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
 * One mixing graph for one performance. Its `rtp` and `l16` slots listen on their ports from
 * here until `destroy`; its return stream sends the mix from `start` until `destroy`. Settings
 * made while armed are kept here and written to the engine when `start` swaps the graph in, so
 * arming the next song never touches the live one.
 */
export class Mixer {
  /** Each slot's kind and, for `rtp` and `l16`, the UDP port it listens on; `null` for `push`. */
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind; port: number | null }>;
  readonly #host: MixerHost;
  readonly #native: NativeMixer;
  readonly #events: Emitter<MixerEvents>;
  readonly #params: SlotParamsInput[];
  readonly #guides: Array<GuideInput | null>;
  readonly #backing: Backing | null;
  readonly #offFault: () => void;
  #phase: Phase = "armed";
  #failed = false;

  /**
   * The mixer can no longer play its song: docs/design.md section 5. Fires once, and a mixer
   * being destroyed has no song left to lose.
   */
  readonly #fail = (error: Error) => {
    if (this.#failed || this.#phase === "destroyed") return;
    this.#failed = true;
    this.#events.emit("error", error);
  };

  constructor(
    host: MixerHost,
    kinds: ReadonlyArray<IngestKind>,
    events: Emitter<MixerEvents>,
    backing: BackingConfig | null,
    rtpReturn: RtpReturnInput | null,
  ) {
    this.#host = host;
    this.#events = events;
    this.#native = host.native.createMixer([...kinds], rtpReturn);
    this.slots = kinds.map((kind, index) => ({ index, kind, port: this.#native.port(index) }));
    this.#params = kinds.map(defaultSlotParams);
    this.#guides = kinds.map(() => null);
    this.#backing =
      backing === null
        ? null
        : new Backing(host, backing, this.#native, {
            progress: (fraction) => {
              if (this.#phase !== "destroyed") this.#events.emit("renderProgress", fraction);
            },
            failed: this.#fail,
          });
    this.#offFault = host.onFault(this.#fail);
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

  /**
   * Forget the stream a slot was hearing; the next packets, or the next pushed audio, are a new
   * singer. An armed mixer has heard nothing yet, so there is nothing to forget.
   */
  resetSlot(index: number) {
    const slot = this.#slot(index);
    if (this.#host.isLive(this)) {
      this.#host.native.resetSlot(slot);
    }
  }

  /**
   * Hands interleaved stereo PCM to a push slot, folded to mono as every slot is. Returns the
   * frames taken; the rest did not fit and is dropped, since late audio is worse than lost audio.
   * Throws on a slot of another kind.
   */
  push(index: number, stereo: Float32Array) {
    const slot = this.#slot(index);
    const kind = this.slots[slot]?.kind;
    if (kind !== "push") {
      throw new Error(`slot ${slot} is ${String(kind)}; only a push slot takes pushed audio`);
    }
    if (this.#phase === "destroyed") {
      throw new Error("the mixer is destroyed");
    }
    return this.#native.push(slot, pcm("stereo", stereo, 2));
  }

  /**
   * Waits for the backing's render and swaps the graph in; a render that failed rejects with the
   * message `error` carried. With the render already done, the swap is sent before this returns
   * to the caller. Nothing is written to the engine while another mixer is live, so a refused
   * start leaves the song playing untouched.
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
      this.#backing?.install();
      this.#params.forEach((params, slot) => {
        this.#host.native.setSlotParams(slot, params);
      });
      if (this.#backing) this.#host.native.setStemGains(this.#backing.stemGains);
      swapped = this.#host.swapIn(this, this.#native, this.#events, this.#fail);
    } catch (error) {
      this.#phase = "armed";
      throw error;
    }
    // From here the graph is the audio thread's, so the mixer is live whatever the acks say.
    this.#phase = "live";
    this.#guides.forEach((guide, slot) => {
      if (guide) this.#host.native.setGuide(slot, guide);
    });
    const started = this.#command(() => this.#host.native.start());
    const stopAtFrames = this.#backing?.stopAtFrames;
    const stopping =
      stopAtFrames === undefined
        ? undefined
        : this.#command(() => this.#host.native.stopAt(stopAtFrames, msToFrames(STOP_FADE_MS)));
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
    const landed = await this.#command(() => native.seek(toFrame));
    return { positionMs: framesToMs(landed) };
  }

  /** Resolves once the fade out is done and the playhead holds. */
  async pause() {
    const native = this.#liveBacking();
    await this.#command(() => native.pause());
  }

  async resume() {
    const native = this.#liveBacking();
    const position = await this.#command(() => native.resume());
    return { positionMs: framesToMs(position) };
  }

  /** The live key, on top of the armed one; its crossfade follows the ack. */
  async setBackingKey(semitones: number) {
    const checked = integerIn("semitones", semitones, -12, 12);
    const native = this.#liveBacking();
    await this.#command(() => native.setKey(checked));
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

  /**
   * Each slot's ingest counters as the audio thread last published them, the engine's underruns,
   * and the blocks the return stream dropped, `null` without one.
   */
  stats(): {
    slots: ReadonlyArray<SlotStats>;
    underruns: number;
    rtpReturn: { droppedBlocks: number } | null;
  } {
    return {
      slots: this.#native.stats(),
      underruns: this.#host.underruns(),
      rtpReturn: this.#native.returnStats(),
    };
  }

  /**
   * Closes the slots' ports, stops the return stream, swaps the silent graph in when this one is
   * live, and resolves once this graph is back and dropped.
   */
  async destroy() {
    switch (this.#phase) {
      case "destroyed":
        return;
      case "starting":
        throw new Error("the mixer is starting; await start before destroying it");
      case "armed":
        this.#release();
        return;
      case "live":
        this.#release();
        await this.#host.swapOut(this);
    }
  }

  #release() {
    this.#phase = "destroyed";
    this.#offFault();
    this.#native.release();
  }

  #command(send: () => number) {
    return this.#host.command(send, this.#fail);
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
