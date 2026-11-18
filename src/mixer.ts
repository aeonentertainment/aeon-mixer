import {
  Backing,
  type BackingConfig,
  backingSettings,
  type RenderHost,
  STOP_FADE_MS,
} from "./backing.ts";
import { Bundle } from "./bundle/bundle.ts";
import type { CommandCall } from "./bundle/bundle-event.ts";
import { bundleOutput } from "./bundle/bundle-manifest.ts";
import type { BundleRequest } from "./bundle/bundle-request.ts";
import type { Emitter } from "./emitter.ts";
import type {
  GuideInput,
  NativeMixer,
  RtpReturnInput,
  SlotInput,
  SlotParamsInput,
} from "./native.ts";
import type { SettingsProfile } from "./profile/settings-profile.ts";
import type {
  BackingStem,
  EngineEvents,
  IngestKind,
  MelodyGuide,
  MixerEvents,
  OutputInfo,
  SlotEffect,
  SlotSettings,
  SlotStats,
  StemKind,
} from "./types.ts";
import { toneInput, toneOff } from "./tone.ts";
import { framesToMs, msToFrames } from "./units.ts";
import { finiteIn, integerIn, pcm, RANGES } from "./validate.ts";

/** What a mixer needs from its engine. Internal to the package. */
export interface MixerHost extends RenderHost {
  /** The engine's settings profile: what a call that leaves a value out takes. */
  readonly profile: SettingsProfile;
  /** The output playing now; it changes when the engine moves. */
  output(): OutputInfo;
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
  /** Hears each change of the engine's output until the returned function is called. */
  onOutputChanged(listener: EngineEvents["outputChanged"]): () => void;
  /**
   * Holds `release` until the returned function is called, and calls it when the engine closes:
   * a mixer's capture inputs and ports outlive no engine, whatever state the mixer is in.
   */
  hold(release: () => void): () => void;
  isLive(mixer: Mixer): boolean;
  anyLive(): boolean;
  underruns(): number;
}

type Phase = "armed" | "starting" | "live" | "destroyed";

type Effects = Pick<
  SlotParamsInput,
  "corrector" | "reverb" | "voice" | "cleanup" | "echoCancel" | "howlGuard" | "tone"
>;

/** Every effect off: a slot's chain as it starts, and what `setSlotEffects` builds on. */
function effectsOff(): Effects {
  return {
    corrector: { enabled: false, strength: 0, retuneMs: 0 },
    reverb: { enabled: false, roomSize: 0, mix: 0 },
    voice: { enabled: false, formantSemitones: 0 },
    echoCancel: { enabled: false },
    cleanup: { enabled: false },
    howlGuard: { enabled: false },
    tone: toneOff(),
  };
}

/**
 * One mixing graph for one performance. Its `rtp` and `l16` slots listen on their ports, and its
 * `capture` slots hold their inputs open, from here until `destroy`; its return stream sends the
 * mix from `start` until `destroy`. Settings
 * made while armed are kept here and written to the engine when `start` swaps the graph in, so
 * arming the next song never touches the live one. A mixer asked for a session bundle writes it
 * from here until `destroy`: docs/design.md 8.11.
 */
export class Mixer {
  /** Each slot's kind and, for `rtp` and `l16`, the UDP port it listens on; `null` for the rest. */
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind; port: number | null }>;
  readonly #host: MixerHost;
  readonly #native: NativeMixer;
  readonly #events: Emitter<MixerEvents>;
  readonly #params: SlotParamsInput[];
  readonly #guides: Array<GuideInput | null>;
  readonly #backing: Backing | null;
  readonly #offFault: () => void;
  readonly #letGo: () => void;
  readonly #bundle: Bundle | null;
  readonly #stems: ReadonlyArray<BackingStem>;
  #offOutput = () => {};
  /** The backing position the engine last reported, for the bundle's stats lines. */
  #positionMs: number | null = null;
  #phase: Phase = "armed";
  #failed = false;

  /**
   * The mixer can no longer play its song: docs/design.md section 5. Fires once, and a mixer
   * being destroyed has no song left to lose.
   */
  readonly #fail = (error: Error) => {
    if (this.#failed || this.#phase === "destroyed") return;
    this.#failed = true;
    this.#bundle?.event({ type: "error", message: this.#withoutPaths(error.message) });
    this.#events.emit("error", error);
  };

  constructor(
    host: MixerHost,
    inputs: ReadonlyArray<SlotInput & { kind: IngestKind }>,
    events: Emitter<MixerEvents>,
    backing: BackingConfig | null,
    rtpReturn: RtpReturnInput | null,
    bundle: BundleRequest | null = null,
  ) {
    this.#host = host;
    this.#events = events;
    this.#stems = backing?.stems ?? [];
    // Checked before the native mixer binds its ports and opens its inputs, which a throw after it
    // would leave held with no mixer to destroy
    const { slot, backing: backingDefaults } = host.profile;
    const settings = backing === null ? null : backingSettings(backing, backingDefaults.stemGains);
    this.#native = host.native.createMixer([...inputs], rtpReturn);
    this.#letGo = host.hold(() => this.#release());
    this.slots = inputs.map(({ kind }, index) => ({ index, kind, port: this.#native.port(index) }));
    this.#params = inputs.map(() => ({
      gain: slot.gain,
      pan: slot.pan,
      mute: false,
      solo: false,
      ...effectsOff(),
      guideOffsetMs: 0,
    }));
    this.#guides = inputs.map(() => null);
    this.#backing =
      settings === null
        ? null
        : new Backing(host, settings, this.#native, {
            progress: (fraction) => {
              if (this.#phase !== "destroyed") this.#events.emit("renderProgress", fraction);
            },
            failed: this.#fail,
            installed: (frames) => this.#bundle?.event({ type: "backingRendered", frames }),
          });
    this.#offFault = host.onFault(this.#fail);
    try {
      this.#bundle = bundle === null ? null : this.#startBundle(bundle);
    } catch (error) {
      this.#release();
      throw error;
    }
  }

  setSlot(index: number, settings: Partial<SlotSettings>) {
    const slot = this.#slot(index);
    const current = this.#paramsOf(slot);
    const record = this.#bundle?.stamp();
    this.#write(slot, {
      ...current,
      gain:
        settings.gain === undefined
          ? current.gain
          : finiteIn("gain", settings.gain, ...RANGES.gain),
      pan: settings.pan === undefined ? current.pan : finiteIn("pan", settings.pan, ...RANGES.pan),
      mute: settings.mute ?? current.mute,
      solo: settings.solo ?? current.solo,
    });
    record?.({ type: "setSlot", index: slot, settings });
  }

  /**
   * The slot's whole chain: an effect not in `effects` is disabled, and a parameter an effect
   * leaves out is the profile's.
   */
  setSlotEffects(index: number, effects: ReadonlyArray<SlotEffect>) {
    const slot = this.#slot(index);
    const next = { ...this.#paramsOf(slot), ...effectsOff() };
    const defaults = this.#host.profile.slot.effects;
    const record = this.#bundle?.stamp();
    for (const effect of effects) {
      switch (effect.kind) {
        case "pitch-correction": {
          const { strength, retuneMs } = defaults["pitch-correction"];
          next.corrector = {
            enabled: effect.enabled,
            strength: finiteIn("strength", effect.strength ?? strength, ...RANGES.strength),
            retuneMs: finiteIn("retuneMs", effect.retuneMs ?? retuneMs, ...RANGES.retuneMs),
          };
          break;
        }
        case "reverb": {
          const { roomSize, mix } = defaults.reverb;
          next.reverb = {
            enabled: effect.enabled,
            roomSize: finiteIn("roomSize", effect.roomSize ?? roomSize, ...RANGES.roomSize),
            mix: finiteIn("mix", effect.mix ?? mix, ...RANGES.mix),
          };
          break;
        }
        case "voice":
          next.voice = {
            enabled: effect.enabled,
            formantSemitones: finiteIn(
              "formantSemitones",
              effect.formantSemitones ?? defaults.voice.formantSemitones,
              ...RANGES.formantSemitones,
            ),
          };
          break;
        case "echo-cancel":
          next.echoCancel = { enabled: effect.enabled };
          break;
        case "cleanup":
          next.cleanup = { enabled: effect.enabled };
          break;
        case "howl-guard":
          next.howlGuard = { enabled: effect.enabled };
          break;
        case "tone":
          next.tone = toneInput(effect);
          break;
      }
    }
    this.#write(slot, next);
    record?.({ type: "setSlotEffects", index: slot, effects });
  }

  setSlotGuide(index: number, guide: MelodyGuide | null, guideOffsetMs?: number) {
    const slot = this.#slot(index);
    const input = guide === null ? null : toGuideInput(guide);
    const record = this.#bundle?.stamp();
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
    record?.({
      type: "setSlotGuide",
      index: slot,
      guide,
      ...(guideOffsetMs === undefined ? {} : { guideOffsetMs }),
    });
  }

  /**
   * Forget the stream a slot was hearing; the next packets, or the next pushed audio, are a new
   * singer. Call it whenever a slot's sender changes: the sender it released is turned away until
   * it has been quiet a second. An armed mixer has heard nothing yet, so there is nothing to forget.
   */
  resetSlot(index: number) {
    const slot = this.#slot(index);
    const record = this.#bundle?.stamp();
    if (this.#host.isLive(this)) {
      this.#host.native.resetSlot(slot);
    }
    record?.({ type: "resetSlot", index: slot });
  }

  /**
   * Writes every datagram that reaches an `rtp` or `l16` slot's port to a new pcap file at `path`,
   * with the moment the network thread read it, until `null`, another path or `destroy`. The file
   * is created here and never over an existing one, so a bad path throws. A recording holds a
   * singer's voice and their phone's LAN address: it is made only when a person asks for one, and
   * nothing in this package sends it anywhere. Start it after `start`, so a replay begins where
   * the live slot did. docs/design.md 8.4. A mixer writing a session bundle already records the
   * slot there, and a slot has one recording, so this throws.
   */
  recordSlot(index: number, path: string | null) {
    const slot = this.#slot(index);
    const kind = this.slots[slot]?.kind;
    if (kind !== "rtp" && kind !== "l16") {
      throw new Error(`slot ${slot} is a ${String(kind)} slot; only rtp and l16 slots record`);
    }
    if (this.#phase === "destroyed") {
      throw new Error("the mixer is destroyed");
    }
    if (this.#bundle) throw new Error(`slot ${slot} is recording into the mixer's bundle`);
    if (path === "") throw new RangeError("path must name a file");
    this.#native.recordSlot(slot, path, false);
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
    let record: ReturnType<Bundle["stamp"]> | undefined;
    try {
      if (this.#backing?.rendering) await this.#backing.rendered;
      if (this.#host.anyLive()) throw new Error("another mixer is live; destroy it first");
      record = this.#bundle?.stamp();
      this.#backing?.install();
      this.#params.forEach((params, slot) => {
        this.#host.native.setSlotParams(slot, params);
      });
      if (this.#backing) this.#host.native.setStemGains(this.#backing.stemGains);
      swapped = this.#host.swapIn(this, this.#native, this.#events, this.#fail);
    } catch (error) {
      if (this.#phase === "starting") this.#phase = "armed";
      throw error;
    }
    // From here the graph is the audio thread's, so the mixer is live whatever the acks say.
    this.#phase = "live";
    record?.({ type: "start" });
    this.#guides.forEach((guide, slot) => {
      if (guide) this.#host.native.setGuide(slot, guide);
    });
    const started = this.#command(() => this.#host.native.start());
    const stopAtFrames = this.#backing?.stopAtFrames;
    const stopping =
      stopAtFrames === undefined
        ? undefined
        : this.#command(() => this.#host.native.stopAt(stopAtFrames, msToFrames(STOP_FADE_MS)));
    const [, position] = await Promise.all([swapped, started, stopping]);
    this.#bundle?.event({ type: "ack", call: "start", positionMs: this.#heard(position) });
    const { latencyMs } = this.#host.output();
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
    const positionMs = await this.#steer(() => native.seek(toFrame), { type: "seek", toMs });
    return { positionMs };
  }

  /** Resolves once the fade out is done and the playhead holds. */
  async pause() {
    const native = this.#liveBacking();
    await this.#steer(() => native.pause(), { type: "pause" });
  }

  async resume() {
    const native = this.#liveBacking();
    const positionMs = await this.#steer(() => native.resume(), { type: "resume" });
    return { positionMs };
  }

  /** The live key, on top of the armed one; its crossfade follows the ack. */
  async setBackingKey(semitones: number) {
    const checked = integerIn("semitones", semitones, -12, 12);
    const native = this.#liveBacking();
    await this.#steer(() => native.setKey(checked), { type: "setBackingKey", semitones });
  }

  /** A parameter write, held while armed like the slot settings. */
  setStemGain(kind: StemKind, gain: number) {
    if (!this.#backing) throw new Error("this mixer has no backing");
    const record = this.#bundle?.stamp();
    const gains = this.#backing.setStemGain(kind, gain);
    if (this.#host.isLive(this)) {
      this.#host.native.setStemGains(gains);
    }
    record?.({ type: "setStemGain", kind, gain });
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
   * live, and resolves once this graph is back and dropped. A bundle's `end` line is written
   * here, last.
   */
  async destroy() {
    switch (this.#phase) {
      case "destroyed":
        return;
      case "starting":
        // Once `error` has fired, the `start` still waiting can only reject, so a host may destroy
        // the mixer from its listener as section 5 asks
        if (!this.#failed) {
          throw new Error("the mixer is starting; await start before destroying it");
        }
        this.#release();
        return;
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
    this.#offOutput();
    this.#letGo();
    this.#bundle?.end();
    this.#native.release();
  }

  /**
   * Starts the bundle, whose pcaps the native mixer writes, and tells it what the engine reports
   * from here on.
   */
  #startBundle({ dir, backing }: BundleRequest) {
    const bundle = new Bundle(dir, backing, {
      profile: this.#host.profile,
      output: this.#host.output(),
      slots: this.slots,
      clock: () => this.#native.clock(),
      record: (slot, path) => {
        this.#native.recordSlot(slot, path, true);
      },
      stats: () => ({ positionMs: this.#positionMs, ...this.stats() }),
    });
    this.#offOutput = this.#host.onOutputChanged((output, gapMs, heldMs) => {
      bundle.event({ type: "outputChanged", output: bundleOutput(output), gapMs, heldMs });
    });
    this.#events.on("position", (positionMs) => {
      this.#positionMs = positionMs;
    });
    this.#events.on("backingEnded", () => {
      bundle.event({ type: "backingEnded" });
    });
    return bundle;
  }

  /**
   * A failed render's message names a file stem by path, and a bytes stem by kind already. A
   * bundle holds no path, so by kind.
   */
  #withoutPaths(message: string) {
    return this.#stems.reduce(
      (text, { kind, path }) => (path === undefined ? text : text.replaceAll(path, kind)),
      message,
    );
  }

  #command(send: () => number) {
    return this.#host.command(send, this.#fail);
  }

  /**
   * Sends a backing command and resolves with the position its ack carries, in milliseconds. A
   * bundle's line for it is written as the command is sent, and its ack's as the ack arrives.
   */
  async #steer(send: () => number, call: CommandCall) {
    const record = this.#bundle?.stamp();
    const acked = this.#command(send);
    record?.(call);
    const positionMs = this.#heard(await acked);
    this.#bundle?.event({ type: "ack", call: call.type, positionMs });
    return positionMs;
  }

  /** Keeps the backing position an ack carried, and returns it in milliseconds. */
  #heard(positionFrames: number) {
    this.#positionMs = framesToMs(positionFrames);
    return this.#positionMs;
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
