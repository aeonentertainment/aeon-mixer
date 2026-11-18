// The public types of docs/design.md 7.2 that are inputs to the API. What the API returns is
// inferred from the implementation.

import type { SettingsProfile } from "./profile/settings-profile.ts";

export type Capability =
  | "guideVocal"
  | "key"
  | "speed"
  | "seek"
  | "shortVersion"
  | "pause"
  | "level";
export type IngestKind = "rtp" | "l16" | "push" | "capture";

/**
 * How a device reaches the machine, as its OS or driver reports it; decision 0025 maps each OS.
 * `virtual` has no hardware behind it (BlackHole, ZoomAudioDevice, a virtual cable): it starts
 * and plays to nobody. `network` plays somewhere else, seconds late. `other` is hardware on a bus
 * not named here; `unknown` is a device the OS or driver did not say. A new OS transport maps into
 * one of these, so the union does not grow.
 */
export type Transport =
  | "builtIn"
  | "usb"
  | "bluetooth"
  | "display"
  | "network"
  | "aggregate"
  | "virtual"
  | "other"
  | "unknown";

/** One device as `outputDevices()` and `inputDevices()` list it. */
export interface DeviceInfo {
  id: string;
  name: string;
  isDefault: boolean;
  transport: Transport;
}
export type StemKind = "instrumental" | "guideVocals";

export interface OpenEngineOptions {
  /**
   * A device, pinned by `device` or, left out, following the OS default; `timer` for a box with
   * no sound card, whose own thread renders on the OS's clock and whose mix leaves only on a
   * mixer's `rtpReturn`; `offline` for tests, which pump it.
   */
  output: { device?: string; periodFrames?: 64 | 128 } | { timer: true } | { offline: true };
  /** Fixed for the engine's life, default 8, at most 16. */
  slots?: number;
  /**
   * Pins the ingest jitter buffer to this delay. Left out, the profile's `ingest.playoutDelayMs`:
   * by default each slot chooses its own from what its stream shows, opening at 15, and `stats()`
   * reports where each one is.
   */
  playoutDelayMs?: number;
  /**
   * The settings nobody chose in the moment. Left out, `defaultProfile`. `output.periodFrames` and
   * `playoutDelayMs` above win over it.
   */
  profile?: SettingsProfile;
}

export interface OutputInfo {
  readonly sampleRate: 48000;
  readonly periodFrames: number;
  readonly latencyMs: number;
  /** The output playing now, as `outputDevices()` lists it; null on the offline clock. */
  readonly device: { readonly id: string; readonly name: string } | null;
  /**
   * The output the engine should play on, the pinned one or the default, when it would not start,
   * with what the OS said. The engine stays on `device`, and the refusal stands, until the pin or
   * default rule picks another output, as when the default changes or the refused output leaves
   * the list. Null while it plays where it should.
   */
  readonly refused: {
    readonly id: string;
    readonly name: string;
    readonly error: string;
  } | null;
}

/**
 * One slot as a mixer asks for it. A `capture` slot is an input on this machine, a house mic:
 * `device` is an id from `inputDevices()`, the default input when left out, and `channel` the
 * device's channel from 0, default 0.
 */
export type SlotConfig =
  | { kind: Exclude<IngestKind, "capture"> }
  | { kind: "capture"; device?: string; channel?: number };

export interface MixerConfig {
  slots: ReadonlyArray<SlotConfig>;
  backing?: {
    stems: ReadonlyArray<{ kind: StemKind; path: string }>;
    /** Semitones, rendered before start. */
    key: number;
    /** A factor, rendered before start. */
    speed: number;
    /** A stem left out takes the profile's gain. */
    stemGains?: Partial<Record<StemKind, number>>;
    stopAtMs?: number;
  };
  rtpReturn?: { host: string; port: number; payloadType: number; ssrc: number };
  /**
   * Writes this mixer's session bundle into a new directory at `dir`, from now until `destroy`:
   * docs/design.md 8.11. `createMixer` rejects when `dir` exists. A bundle holds every singer's
   * voice in the song: nothing in this package sends it anywhere, and the host owns it and
   * answers for each singer's consent.
   */
  bundle?: { dir: string };
}

export interface SlotSettings {
  /** Linear amplitude, 0 to 4. */
  gain: number;
  /** -1 left to 1 right. */
  pan: number;
  mute: boolean;
  solo: boolean;
}

/** A parameter left out takes the profile's value. */
export type SlotEffect =
  | { kind: "pitch-correction"; enabled: boolean; strength?: number; retuneMs?: number }
  | { kind: "reverb"; enabled: boolean; roomSize?: number; mix?: number }
  /**
   * A formant move in semitones from the singer's own, -12 to 12, with no change of pitch and no
   * added delay. Decision 0014 has the first voices' settings.
   */
  | { kind: "voice"; enabled: boolean; formantSemitones?: number }
  /** Takes the room's echo of the mix out of the slot's input, with no added delay. */
  | { kind: "echo-cancel"; enabled: boolean }
  /**
   * A high-pass and gate ahead of the corrector and a compressor after the meter. None reads
   * ahead or buffers; decision 0012 has their numbers and costs.
   */
  | { kind: "cleanup"; enabled: boolean }
  /** Ducks the slot when its loop through the speakers starts to howl, with no added delay. */
  | { kind: "howl-guard"; enabled: boolean }
  /**
   * Makes a phone sound like a good vocal mic and holds no sample. `bands` is the host's
   * correction for the mic it knows, at most 8. `plosiveDb` 0 to 12, `airDb` 0 to 6 and `deEssDb`
   * 0 to 12 are amounts; they are not in the profile, and left out each is 0 and its part is out
   * of the path. Throws when the bands and the air together leave a frequency from 80 Hz over
   * 0.2 ms late or lift one over 6 dB. `"match"`, the blind match for a mic with no bands, throws
   * too: it has no target curve to aim at yet. Decision 0029 has the reasons and numbers.
   */
  | {
      kind: "tone";
      enabled: boolean;
      bands?: ReadonlyArray<ToneBand> | "match";
      plosiveDb?: number;
      airDb?: number;
      deEssDb?: number;
    };

/**
 * One filter of a tone stage's `bands`: the Audio EQ Cookbook's peaking or shelf biquad at 48 kHz,
 * a shelf at slope 1, so a host's fitting tool draws the same curve from the same numbers. `hz`
 * 80 to 16 000, `gainDb` -12 to 6, `q` 0.3 to 8.
 */
export type ToneBand =
  | { kind: "peak"; hz: number; gainDb: number; q: number }
  | { kind: "low-shelf" | "high-shelf"; hz: number; gainDb: number };

export interface MelodyGuide {
  notes: ReadonlyArray<{ midi: number; startMs: number; endMs: number }>;
  /** The key's pitch classes as bits, C in bit 0. */
  keyMask: number;
}

export interface SlotStats {
  pushed: number;
  lost: number;
  late: number;
  /** Of `late`, the packets that would also have been late had the delay stayed where it opened. */
  lateAtOpening: number;
  concealed: number;
  resyncs: number;
  jitterMs: number;
  /**
   * How far the corrector holds this slot's voice now; 0 with correction off. It moves with the
   * voice and one read is a sample of it, so show it and never add it to a budget or a clock.
   */
  correctorDelayMs: number;
  /** The most that delay can reach; 0 with correction off. It holds still, so a budget adds this. */
  correctorMaxDelayMs: number;
  /**
   * What the echo canceller found: `pathMs` from the mix being rendered to its echo reaching the
   * slot, `bleedDb` how far under the mix that echo arrives, `removedDb` how much quieter the slot
   * came out than it went in, both over the last second. Null while it is off, or until it has
   * found the echo and learned it.
   */
  echo: { pathMs: number; bleedDb: number; removedDb: number } | null;
  /**
   * The delay the stream's earliest packets wait now; zero for a `push` slot. A `capture` slot
   * has no jitter buffer: this is how long its mic's samples wait from the input to the mix, the
   * input's period included, and zero until it first plays.
   */
  playoutDelayMs: number;
  /** How often the howl guard has ducked this slot. */
  howlCatches: number;
  /**
   * What the tone stage did; null while it is off. `match` is null until the blind match ships.
   * `deEssDb` is the most the de-esser took off in the last second, `plosives` the pops the guard
   * has ducked.
   */
  tone: {
    match: { bands: ReadonlyArray<{ hz: number; gainDb: number }>; countedMs: number } | null;
    deEssDb: number;
    plosives: number;
  } | null;
}

export type EngineEvents = {
  fault: (error: Error) => void;
  /**
   * Whenever `output` changes: after every move, `gapMs` how long the room heard nothing and
   * `heldMs` how long that held the backing's playhead, `gapMs` while it was playing or fading out
   * to a pause or a seek and 0 while it was not; both 0 when `refused` clears with no move. The
   * gap starts where the old output stopped asking for audio, so after a stall it counts the
   * stall. A host's lyric clock moves by `heldMs` and the change in `latencyMs`.
   */
  outputChanged: (output: OutputInfo, gapMs: number, heldMs: number) => void;
  /** The mix after the clipper, reactions included, between songs too. */
  masterLevel: (rmsDb: number, peakDb: number) => void;
};

export type MixerEvents = {
  level: (slot: number, rmsDb: number, peakDb: number) => void;
  /**
   * Every 10 ms, on the guide's clock. `targetMidi` is the guide note the corrector pulls toward,
   * after the backing's key shift: null with correction off, with no guide, or between notes,
   * where it pulls toward the key. `errorSemitones` is the class it pulls toward less the
   * reading, in the singer's own octave, so positive is flat: 0 with correction off or while
   * unvoiced.
   */
  pitch: (
    slot: number,
    positionMs: number,
    hz: number,
    confidence: number,
    targetMidi: number | null,
    errorSemitones: number,
  ) => void;
  position: (positionMs: number) => void;
  renderProgress: (fraction: number) => void;
  backingEnded: () => void;
  error: (error: Error) => void;
};
