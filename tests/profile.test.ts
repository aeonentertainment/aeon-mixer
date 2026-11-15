import { expect, test } from "vite-plus/test";
import {
  defaultProfile,
  type Mixer,
  type OfflineEngine,
  openEngine,
  parseProfile,
  type SettingsProfile,
  type SlotEffect,
} from "../src/index.ts";

const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;

async function startLive(engine: OfflineEngine, mixer: Mixer) {
  const started = mixer.start();
  engine.pump(FADE_BLOCKS);
  return started;
}

async function destroyLive(engine: OfflineEngine, mixer: Mixer) {
  const destroyed = mixer.destroy();
  engine.pump(FADE_BLOCKS);
  await destroyed;
}

/** Interleaved stereo holding a 440 Hz tone, which folds to that tone. */
function tone(blocks: number) {
  return Float32Array.from({ length: 2 * BLOCK * blocks }, (_, i) =>
    Math.fround(0.5 * Math.sin((2 * Math.PI * 440 * Math.floor(i / 2)) / 48000)),
  );
}

/** What one push slot plays of a tone under `profile`, after `effects` are set on it. */
async function played(profile: SettingsProfile, effects: ReadonlyArray<SlotEffect>) {
  const engine = await openEngine({ output: { offline: true }, profile });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  mixer.setSlotEffects(0, effects);
  await startLive(engine, mixer);
  mixer.push(0, tone(16));
  const out = engine.pump(16);
  await destroyLive(engine, mixer);
  await engine.close();
  return out;
}

/** A copy of the default profile for a test to change. */
function profileWith(change: (profile: SettingsProfile) => void) {
  const profile = structuredClone(defaultProfile);
  change(profile);
  return profile;
}

/** The delay an `rtp` slot of `engine` holds before any packet arrives. */
async function delayOf(engine: OfflineEngine) {
  const mixer = await engine.createMixer({ slots: [{ kind: "rtp" }] });
  return mixer.stats().slots[0]?.playoutDelayMs;
}

test("an engine opened with no profile runs under the default one", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(engine.profile).toEqual({
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
  await engine.close();
});

test("the profile pins the playout delay, the option wins over it, and the profile reads back the result", async () => {
  const profile = profileWith((it) => {
    it.ingest.playoutDelayMs = 30;
  });
  const fromProfile = await openEngine({ output: { offline: true }, profile });
  expect(fromProfile.profile.ingest.playoutDelayMs).toBe(30);
  expect(await delayOf(fromProfile)).toBe(30);
  await fromProfile.close();

  const fromOption = await openEngine({ output: { offline: true }, profile, playoutDelayMs: 20 });
  expect(fromOption.profile).toEqual({ ...profile, ingest: { playoutDelayMs: 20 } });
  expect(await delayOf(fromOption)).toBe(20);
  await fromOption.close();
});

test("the profile in force is the engine's own: the host's object can change, and the engine's cannot", async () => {
  const profile = profileWith((it) => {
    it.slot.gain = 0.5;
  });
  const engine = await openEngine({ output: { offline: true }, profile });
  profile.slot.gain = 2;
  expect(engine.profile.slot.gain).toBe(0.5);
  expect(() => {
    engine.profile.slot.gain = 2;
  }).toThrow(TypeError);
  await engine.close();
});

test("a slot starts at the profile's gain and pan", async () => {
  const profile = profileWith((it) => {
    it.slot.gain = 0.5;
    it.slot.pan = -1;
  });
  const engine = await openEngine({ output: { offline: true }, profile });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  await startLive(engine, mixer);
  mixer.push(0, new Float32Array(2 * BLOCK).fill(0.5));
  const out = engine.pump(1);
  out.forEach((sample, i) => {
    expect(sample).toBeCloseTo(i % 2 === 0 ? 0.25 : 0, 5);
  });
  await destroyLive(engine, mixer);
  await engine.close();
});

test("an effect parameter a call leaves out is the profile's, and one it names is the call's", async () => {
  const roomy = profileWith((it) => {
    it.slot.effects.reverb = { roomSize: 0.9, mix: 0.6 };
  });
  const leftOut = await played(roomy, [{ kind: "reverb", enabled: true }]);
  const named = await played(defaultProfile, [
    { kind: "reverb", enabled: true, roomSize: 0.9, mix: 0.6 },
  ]);
  expect(leftOut).toEqual(named);

  // A host that names every value hears its own, whatever the profile holds
  const defaults = await played(defaultProfile, [{ kind: "reverb", enabled: true }]);
  const choice: SlotEffect = { kind: "reverb", enabled: true, roomSize: 0.5, mix: 0.25 };
  expect(await played(roomy, [choice])).toEqual(defaults);
  expect(await played(defaultProfile, [choice])).toEqual(defaults);
  expect(leftOut).not.toEqual(defaults);

  // One parameter named, the other left out
  expect(await played(roomy, [{ kind: "reverb", enabled: true, mix: 0.25 }])).toEqual(
    await played(defaultProfile, [{ kind: "reverb", enabled: true, roomSize: 0.9, mix: 0.25 }]),
  );
});

test("parseProfile reads a profile back from JSON", () => {
  expect(parseProfile(JSON.parse(JSON.stringify(defaultProfile)))).toEqual(defaultProfile);
  const pinned = profileWith((it) => {
    it.output.periodFrames = 64;
    it.ingest.playoutDelayMs = 12.5;
  });
  expect(parseProfile(JSON.parse(JSON.stringify(pinned)))).toEqual(pinned);
});

test("parseProfile refuses a missing field, an unknown field, a value out of range and an unknown version, each by name", () => {
  const { backing: _, ...noBacking } = defaultProfile;
  const { slot } = defaultProfile;
  const { reverb } = slot.effects;
  const withReverb = (value: object) => ({
    ...defaultProfile,
    slot: { ...slot, effects: { ...slot.effects, reverb: value } },
  });
  expect(() => parseProfile(withReverb({ roomSize: 0.5 }))).toThrow(
    "slot.effects.reverb.mix is missing",
  );
  expect(() => parseProfile(noBacking)).toThrow("backing is missing");
  expect(() => parseProfile(withReverb({ ...reverb, wet: 0.3 }))).toThrow(
    "slot.effects.reverb.wet is not a field of a settings profile",
  );
  expect(() => parseProfile({ ...defaultProfile, slot: { ...slot, gain: 5 } })).toThrow(
    "slot.gain must be between 0 and 4, got 5",
  );
  expect(() => parseProfile({ ...defaultProfile, ingest: { playoutDelayMs: 4 } })).toThrow(
    "ingest.playoutDelayMs must be between 5 and 100, got 4",
  );
  expect(() => parseProfile({ ...defaultProfile, output: { periodFrames: 256 } })).toThrow(
    "output.periodFrames must be 64 or 128, got 256",
  );
  expect(() => parseProfile(withReverb({ ...reverb, mix: "0.25" }))).toThrow(
    "slot.effects.reverb.mix must be a finite number, got 0.25",
  );
  // The version is judged first: another version's fields are not this one's to refuse
  expect(() => parseProfile({ ...defaultProfile, version: 2, tuning: {} })).toThrow(
    "version must be 1, got 2",
  );
  expect(() => parseProfile({})).toThrow("version is missing");
  expect(() => parseProfile(null)).toThrow("profile must be an object, got null");
});

test("openEngine refuses a profile parseProfile would", async () => {
  const profile = profileWith((it) => {
    it.slot.pan = 2;
  });
  await expect(openEngine({ output: { offline: true }, profile })).rejects.toThrow(
    "slot.pan must be between -1 and 1, got 2",
  );
});
