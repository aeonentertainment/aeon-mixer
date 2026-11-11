// The public types of docs/design.md 7.2 that are inputs to the API. What the API returns is
// inferred from the implementation.

export type Capability =
  | "guideVocal"
  | "key"
  | "speed"
  | "seek"
  | "shortVersion"
  | "pause"
  | "level";
export type IngestKind = "rtp" | "l16" | "push";
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
  /** The ingest jitter buffer, default 15. */
  playoutDelayMs?: number;
}

export interface OutputInfo {
  readonly sampleRate: 48000;
  readonly periodFrames: number;
  readonly latencyMs: number;
  /** The output playing now, as `outputDevices()` lists it; null on the offline clock. */
  readonly device: { readonly id: string; readonly name: string } | null;
}

export interface MixerConfig {
  slots: ReadonlyArray<{ kind: IngestKind }>;
  backing?: {
    stems: ReadonlyArray<{ kind: StemKind; path: string }>;
    /** Semitones, rendered before start. */
    key: number;
    /** A factor, rendered before start. */
    speed: number;
    stemGains: Record<StemKind, number>;
    stopAtMs?: number;
  };
  rtpReturn?: { host: string; port: number; payloadType: number; ssrc: number };
}

export interface SlotSettings {
  /** Linear amplitude, 0 to 4. */
  gain: number;
  /** -1 left to 1 right. */
  pan: number;
  mute: boolean;
  solo: boolean;
}

export type SlotEffect =
  | { kind: "pitch-correction"; enabled: boolean; strength: number; retuneMs: number }
  | { kind: "reverb"; enabled: boolean; roomSize: number; mix: number };

export interface MelodyGuide {
  notes: ReadonlyArray<{ midi: number; startMs: number; endMs: number }>;
  /** The key's pitch classes as bits, C in bit 0. */
  keyMask: number;
}

export interface SlotStats {
  pushed: number;
  lost: number;
  late: number;
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
}

export type EngineEvents = {
  fault: (error: Error) => void;
  /** After every move to another output; `gapMs` is how long the room heard nothing. */
  outputChanged: (output: OutputInfo, gapMs: number) => void;
  /** The mix after the clipper, reactions included, between songs too. */
  masterLevel: (rmsDb: number, peakDb: number) => void;
};

export type MixerEvents = {
  level: (slot: number, rmsDb: number, peakDb: number) => void;
  pitch: (slot: number, positionMs: number, hz: number, confidence: number) => void;
  position: (positionMs: number) => void;
  renderProgress: (fraction: number) => void;
  backingEnded: () => void;
  error: (error: Error) => void;
};
