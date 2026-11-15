import { finiteIn, outputPeriod, RANGES } from "../validate.ts";
import type { SettingsProfile } from "./settings-profile.ts";

/**
 * A profile from JSON, or from any value a host built. Throws naming the field that is missing,
 * unknown or out of range, or the version it does not know. An unknown field is refused, not
 * ignored, so a misspelt setting never passes as one that did nothing.
 */
export function parseProfile(value: unknown): SettingsProfile {
  const versioned = record("profile", value);
  // First, since another version's fields are not this one's to judge
  if (!("version" in versioned)) throw new TypeError("version is missing");
  if (versioned.version !== 1) {
    throw new RangeError(`version must be 1, got ${String(versioned.version)}`);
  }
  const root = fields("", versioned, ["version", "output", "ingest", "slot", "backing"]);
  const output = fields("output", root("output"), ["periodFrames"]);
  const ingest = fields("ingest", root("ingest"), ["playoutDelayMs"]);
  const slot = fields("slot", root("slot"), ["gain", "pan", "effects"]);
  const effects = fields("slot.effects", slot("effects"), ["pitch-correction", "reverb", "voice"]);
  const corrector = fields("slot.effects.pitch-correction", effects("pitch-correction"), [
    "strength",
    "retuneMs",
  ]);
  const reverb = fields("slot.effects.reverb", effects("reverb"), ["roomSize", "mix"]);
  const voice = fields("slot.effects.voice", effects("voice"), ["formantSemitones"]);
  const backing = fields("backing", root("backing"), ["stemGains"]);
  const stemGains = fields("backing.stemGains", backing("stemGains"), [
    "instrumental",
    "guideVocals",
  ]);
  const delay = ingest("playoutDelayMs");
  return {
    version: 1,
    output: { periodFrames: outputPeriod("output.periodFrames", output("periodFrames")) },
    ingest: {
      playoutDelayMs:
        delay === "auto"
          ? "auto"
          : finiteIn("ingest.playoutDelayMs", delay, ...RANGES.playoutDelayMs),
    },
    slot: {
      gain: finiteIn("slot.gain", slot("gain"), ...RANGES.gain),
      pan: finiteIn("slot.pan", slot("pan"), ...RANGES.pan),
      effects: {
        "pitch-correction": {
          strength: finiteIn(
            "slot.effects.pitch-correction.strength",
            corrector("strength"),
            ...RANGES.strength,
          ),
          retuneMs: finiteIn(
            "slot.effects.pitch-correction.retuneMs",
            corrector("retuneMs"),
            ...RANGES.retuneMs,
          ),
        },
        reverb: {
          roomSize: finiteIn(
            "slot.effects.reverb.roomSize",
            reverb("roomSize"),
            ...RANGES.roomSize,
          ),
          mix: finiteIn("slot.effects.reverb.mix", reverb("mix"), ...RANGES.mix),
        },
        voice: {
          formantSemitones: finiteIn(
            "slot.effects.voice.formantSemitones",
            voice("formantSemitones"),
            ...RANGES.formantSemitones,
          ),
        },
      },
    },
    backing: {
      stemGains: {
        instrumental: finiteIn(
          "backing.stemGains.instrumental",
          stemGains("instrumental"),
          ...RANGES.stemGain,
        ),
        guideVocals: finiteIn(
          "backing.stemGains.guideVocals",
          stemGains("guideVocals"),
          ...RANGES.stemGain,
        ),
      },
    },
  };
}

function record(name: string, value: unknown): object {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object, got ${JSON.stringify(value) ?? String(value)}`);
  }
  return value;
}

/**
 * A reader of the object at `path`, which holds exactly `keys`: a key it lacks, or one not in
 * `keys`, throws by name here.
 */
function fields<K extends string>(path: string, value: unknown, keys: ReadonlyArray<K>) {
  const at = (key: string) => (path === "" ? key : `${path}.${key}`);
  const entries = new Map<string, unknown>(Object.entries(record(path, value)));
  const known = new Set<string>(keys);
  for (const key of entries.keys()) {
    if (!known.has(key)) throw new TypeError(`${at(key)} is not a field of a settings profile`);
  }
  for (const key of keys) {
    if (!entries.has(key)) throw new TypeError(`${at(key)} is missing`);
  }
  return (key: K) => entries.get(key);
}
