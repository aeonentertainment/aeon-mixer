import { deepFreeze } from "./deep-freeze.ts";
import type { SettingsProfile } from "./settings-profile.ts";

/**
 * The profile of an engine opened with none. The effect values are the ones Aeon Karaoke has been
 * sung with: decision 0027. Frozen: spread it to make another.
 */
export const defaultProfile: SettingsProfile = deepFreeze({
  version: 1,
  output: { periodFrames: 128 },
  ingest: { playoutDelayMs: "auto" },
  slot: {
    gain: 1,
    pan: 0,
    effects: {
      "pitch-correction": { strength: 0.6, retuneMs: 40 },
      reverb: { roomSize: 0.5, mix: 0.25 },
      voice: { formantSemitones: 0 },
    },
  },
  backing: { stemGains: { instrumental: 1, guideVocals: 0 } },
});
