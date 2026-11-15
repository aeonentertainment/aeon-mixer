import { finiteIn, outputPeriod, RANGES } from "../validate.ts";
import type { SettingsProfile } from "./settings-profile.ts";

/** Checks the value of the field called `name` and returns it, or throws by that name. */
type Check<T> = (name: string, value: unknown) => T;

/** `T`'s shape with a check at each leaf, so the compiler holds a schema to its type key for key. */
type Schema<T> = { [K in keyof T]-?: T[K] extends object ? Schema<T[K]> : Check<T[K]> };

/** A schema as `walk` reads it, whatever type it checks. */
interface Fields {
  readonly [key: string]: Check<unknown> | Fields;
}

function within([min, max]: readonly [number, number]): Check<number> {
  return (name, value) => finiteIn(name, value, min, max);
}

const version: Check<1> = (name, value) => {
  if (value !== 1) throw new RangeError(`${name} must be 1, got ${String(value)}`);
  return value;
};

/** The profile's one shape for the parser: a field joins here in one line, at the setter's range. */
const SCHEMA: Schema<SettingsProfile> = {
  version,
  output: { periodFrames: outputPeriod },
  ingest: {
    playoutDelayMs: (name, value) =>
      value === "auto" ? value : finiteIn(name, value, ...RANGES.playoutDelayMs),
  },
  slot: {
    gain: within(RANGES.gain),
    pan: within(RANGES.pan),
    effects: {
      "pitch-correction": {
        strength: within(RANGES.strength),
        retuneMs: within(RANGES.retuneMs),
      },
      reverb: { roomSize: within(RANGES.roomSize), mix: within(RANGES.mix) },
      voice: { formantSemitones: within(RANGES.formantSemitones) },
    },
  },
  backing: {
    stemGains: {
      instrumental: within(RANGES.stemGain),
      guideVocals: within(RANGES.stemGain),
    },
  },
};

/**
 * A profile from JSON, or from any value a host built. Throws naming the field that is missing,
 * unknown or out of range, or the version it does not know. An unknown field is refused, not
 * ignored, so a misspelt setting never passes as one that did nothing.
 */
export function parseProfile(value: unknown): SettingsProfile {
  const profile = record("profile", value);
  // First, since another version's fields are not this one's to judge
  if (!("version" in profile)) throw new TypeError("version is missing");
  version("version", profile.version);
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- `SCHEMA` is a `Schema<SettingsProfile>`, so the compiler holds what the walk builds to the type key for key
  return walk("", SCHEMA, profile) as SettingsProfile;
}

function record(name: string, value: unknown): object {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object, got ${JSON.stringify(value) ?? String(value)}`);
  }
  return value;
}

/**
 * The object at `path` read through `schema`: it holds exactly the schema's keys, each one checked
 * under its dotted name.
 */
function walk(path: string, schema: Fields, value: unknown): object {
  const at = (key: string) => (path === "" ? key : `${path}.${key}`);
  const entries = new Map<string, unknown>(Object.entries(record(path, value)));
  for (const key of entries.keys()) {
    if (!Object.hasOwn(schema, key)) {
      throw new TypeError(`${at(key)} is not a field of a settings profile`);
    }
  }
  return Object.fromEntries(
    Object.entries(schema).map(([key, check]) => {
      if (!entries.has(key)) throw new TypeError(`${at(key)} is missing`);
      const field = entries.get(key);
      return [
        key,
        typeof check === "function" ? check(at(key), field) : walk(at(key), check, field),
      ];
    }),
  );
}
