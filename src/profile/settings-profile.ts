import type { StemKind } from "../types.ts";

/**
 * Every setting the engine takes that is not a singer's or a host's choice in the moment:
 * docs/design.md 8.12. A call that names a value is a choice; one that leaves it out takes the
 * profile's. `openEngine` takes one for the engine's life.
 */
export interface SettingsProfile {
  version: 1;
  /** A device engine's period. The timer and offline clocks render blocks of 128 whatever it says. */
  output: { periodFrames: 64 | 128 };
  /** The ingest jitter buffer's delay, 5 to 100 ms, or `"auto"`: each slot chooses its own. */
  ingest: { playoutDelayMs: number | "auto" };
  /** Where a slot starts, and what `setSlotEffects` fills in for a parameter a call leaves out. */
  slot: {
    gain: number;
    pan: number;
    effects: {
      "pitch-correction": { strength: number; retuneMs: number };
      reverb: { roomSize: number; mix: number };
      voice: { formantSemitones: number };
    };
  };
  /** What `createMixer` fills in for a stem gain its config leaves out. */
  backing: { stemGains: Record<StemKind, number> };
}
