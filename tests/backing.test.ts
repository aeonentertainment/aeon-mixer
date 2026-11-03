import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import { type Mixer, type MixerConfig, type OfflineEngine, openEngine } from "../src/index.ts";

const RATE = 48000;
const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
/** Seek and pause fade over 20 ms, 7.5 blocks: the eighth block lands them. */
const SEEK_FADE_BLOCKS = 8;
/** A stem gain ramps over 50 ms, 18.75 blocks. */
const STEM_RAMP_BLOCKS = 19;
const SECOND_BLOCKS = RATE / BLOCK;
const HALF_SECOND_BLOCKS = 188;

const dir = mkdtempSync(join(tmpdir(), "aeon-backing-"));
let written = 0;
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** Writes `samples` as a mono 16-bit WAV at 48 kHz and returns its path. */
function stem(samples: Float32Array) {
  const data = Buffer.alloc(samples.length * 2);
  samples.forEach((sample, i) => {
    data.writeInt16LE(Math.round(Math.max(-1, Math.min(1, sample)) * 32767), 2 * i);
  });
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(RATE, 24);
  header.writeUInt32LE(RATE * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  written += 1;
  const path = join(dir, `stem-${written}.wav`);
  writeFileSync(path, Buffer.concat([header, data]));
  return path;
}

function constant(level: number, seconds: number) {
  return new Float32Array(RATE * seconds).fill(level);
}

function sine(hz: number, seconds: number) {
  return Float32Array.from(
    { length: RATE * seconds },
    (_, i) => 0.5 * Math.sin((2 * Math.PI * hz * i) / RATE),
  );
}

/** Twelve harmonics falling as 1/h at `hz`: a tone the stretcher treats as music, as the engine's tests use. */
function harmonicTone(hz: number, seconds: number) {
  return Float32Array.from({ length: RATE * seconds }, (_, i) => {
    let sample = 0;
    for (let h = 1; h <= 12; h += 1) sample += Math.sin((2 * Math.PI * hz * h * i) / RATE) / h;
    return 0.25 * sample;
  });
}

/** Every sample names its frame: the frame over the length, halved. */
function ramp(seconds: number) {
  const frames = RATE * seconds;
  return Float32Array.from({ length: frames }, (_, i) => (0.5 * i) / frames);
}

const unity = { instrumental: 1, guideVocals: 1 };

type BackingConfig = NonNullable<MixerConfig["backing"]>;

function backing(
  instrumental: Float32Array,
  options: Partial<Omit<BackingConfig, "stems">> & { guide?: Float32Array } = {},
): BackingConfig {
  const { guide, key = 0, speed = 1, stemGains = unity, ...rest } = options;
  return {
    stems: [
      { kind: "instrumental", path: stem(instrumental) },
      ...(guide === undefined ? [] : [{ kind: "guideVocals" as const, path: stem(guide) }]),
    ],
    key,
    speed,
    stemGains,
    ...rest,
  };
}

function left(interleaved: Float32Array) {
  return Array.from(interleaved.filter((_, i) => i % 2 === 0));
}

/**
 * The pitch of one window: the lag within a semitone of `nearHz` where the normalised
 * autocorrelation peaks, refined by a parabola through its neighbours.
 */
function pitchOf(window: number[], nearHz: number) {
  const correlation = (lag: number) => {
    let sum = 0;
    let early = 0;
    let late = 0;
    for (let i = 0; i + lag < window.length; i += 1) {
      const x = window[i] ?? 0;
      const y = window[i + lag] ?? 0;
      sum += x * y;
      early += x * x;
      late += y * y;
    }
    return sum / Math.sqrt(early * late);
  };
  const period = RATE / nearHz;
  let best = { lag: 0, score: -1 };
  for (let lag = Math.floor(period * 0.94); lag <= Math.ceil(period / 0.94); lag += 1) {
    const score = correlation(lag);
    if (score > best.score) best = { lag, score };
  }
  const before = correlation(best.lag - 1);
  const after = correlation(best.lag + 1);
  return RATE / (best.lag + (0.5 * (before - after)) / (before - 2 * best.score + after));
}

/** The median pitch over every 10 ms step of `samples`, as the engine's tests read a tone. */
function medianPitch(samples: number[], nearHz: number) {
  const window = 2048;
  const readings: number[] = [];
  for (let end = window; end <= samples.length; end += RATE / 100) {
    readings.push(pitchOf(samples.slice(end - window, end), nearHz));
  }
  readings.sort((a, b) => a - b);
  const middle = readings[readings.length >> 1];
  if (middle === undefined) throw new Error("no tone");
  return middle;
}

function cents(hz: number, reference: number) {
  return 1200 * Math.log2(hz / reference);
}

function oncePosition(mixer: Mixer) {
  return new Promise<number>((resolve) => {
    const off = mixer.on("position", (positionMs) => {
      off();
      resolve(positionMs);
    });
  });
}

function onceEnded(mixer: Mixer) {
  return new Promise<void>((resolve) => {
    const off = mixer.on("backingEnded", () => {
      off();
      resolve();
    });
  });
}

/** Every render progress fraction, resolving with them once the last one is 1. */
function rendered(mixer: Mixer) {
  const fractions: number[] = [];
  return new Promise<number[]>((resolve) => {
    mixer.on("renderProgress", (fraction) => {
      fractions.push(fraction);
      if (fraction === 1) resolve(fractions);
    });
  });
}

/** Starts a rendered mixer, pumping the offline clock through the swap fade so the acks arrive. */
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

test("the render reports its progress to 1, then start resolves the offset and the speed", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [],
    backing: backing(sine(440, 1), { speed: 1.09 }),
  });
  const fractions = await rendered(mixer);
  expect(fractions.at(-1)).toBe(1);
  expect(fractions.every((f, i) => f >= 0 && f <= (fractions[i + 1] ?? 1))).toBe(true);

  const started = await startLive(engine, mixer);
  expect(started).toMatchObject({ backingOffsetMs: 0, speedFactor: 1.09 });
  const ended = onceEnded(mixer);
  // At 1.09 the second of tone lasts 917 ms: still playing at 850 ms, over by 950 ms
  engine.pump(319 - FADE_BLOCKS);
  expect(engine.pump(1).some((sample) => Math.abs(sample) > 0.1)).toBe(true);
  engine.pump(37);
  await ended;
  await destroyLive(engine, mixer);
  await engine.close();
});

test("start called before the render is done waits for it, and the armed key is heard", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [],
    backing: backing(harmonicTone(220, 3), { key: 2 }),
  });
  const started = mixer.start();
  await rendered(mixer);
  engine.pump(FADE_BLOCKS);
  expect(await started).toMatchObject({ backingOffsetMs: 0, speedFactor: 1 });

  engine.pump(HALF_SECOND_BLOCKS);
  const heard = left(engine.pump(SECOND_BLOCKS));
  const expected = 220 * 2 ** (2 / 12);
  expect(Math.abs(cents(medianPitch(heard, expected), expected))).toBeLessThan(5);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a seek acks the position it landed on and plays on from there; position ticks each second", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const frames = 3 * RATE;
  const mixer = await engine.createMixer({ slots: [], backing: backing(ramp(3)) });
  await rendered(mixer);
  await startLive(engine, mixer);

  const position = oncePosition(mixer);
  engine.pump(SECOND_BLOCKS - FADE_BLOCKS);
  expect(await position).toBe(1000);

  const seeking = mixer.seek(2000);
  engine.pump(SEEK_FADE_BLOCKS);
  expect(await seeking).toEqual({ positionMs: 2000 });
  engine.pump(SEEK_FADE_BLOCKS);
  const heard = left(engine.pump(1));
  const landed = ((heard[0] ?? 0) / 0.5) * frames;
  expect(Math.abs(landed - (2 * RATE + SEEK_FADE_BLOCKS * BLOCK))).toBeLessThan(BLOCK);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("pause holds the playhead in silence and resume plays on from where it held", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backing(constant(0.5, 2)) });
  await rendered(mixer);
  await startLive(engine, mixer);
  engine.pump(10);

  const pausing = mixer.pause();
  engine.pump(SEEK_FADE_BLOCKS);
  await pausing;
  expect(engine.pump(4).every((sample) => sample === 0)).toBe(true);

  const resuming = mixer.resume();
  engine.pump(1);
  const held = (FADE_BLOCKS + 10 + SEEK_FADE_BLOCKS) * BLOCK;
  expect(await resuming).toEqual({ positionMs: held / 48 });
  engine.pump(SEEK_FADE_BLOCKS - 1);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.5) < 1e-3)).toBe(true);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a start refused by a live mixer leaves it untouched, and succeeds once it is gone", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const live = await engine.createMixer({ slots: [], backing: backing(constant(0.5, 2)) });
  await rendered(live);
  await startLive(engine, live);
  const next = await engine.createMixer({
    slots: [],
    backing: backing(constant(0.5, 2), { stemGains: { instrumental: 0.5, guideVocals: 1 } }),
  });
  await rendered(next);
  await expect(next.start()).rejects.toThrow(/another mixer is live/);
  engine.pump(STEM_RAMP_BLOCKS);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.5) < 1e-3)).toBe(true);

  await destroyLive(engine, live);
  await startLive(engine, next);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.25) < 1e-3)).toBe(true);
  await destroyLive(engine, next);
  await engine.close();
});

test("a stem gain is held while armed, then ramps over 50 ms when changed live", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [],
    backing: backing(constant(0.25, 2), {
      guide: constant(0.25, 2),
      stemGains: { instrumental: 1, guideVocals: 0.5 },
    }),
  });
  mixer.setStemGain("instrumental", 0.5);
  await rendered(mixer);
  await startLive(engine, mixer);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.25) < 1e-3)).toBe(true);

  mixer.setStemGain("guideVocals", 0);
  const ramping = left(engine.pump(1));
  expect(ramping.every((sample) => sample > 0.125 && sample < 0.25)).toBe(true);
  engine.pump(STEM_RAMP_BLOCKS - 1);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.125) < 1e-3)).toBe(true);

  const ended = onceEnded(mixer);
  engine.pump(2 * SECOND_BLOCKS);
  await ended;
  await destroyLive(engine, mixer);
  await engine.close();
});

test("stopAtMs fades the stems out, is silent at its frame, and raises backingEnded once", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [],
    backing: backing(constant(0.5, 3), { stopAtMs: 1000 }),
  });
  await rendered(mixer);
  let endings = 0;
  mixer.on("backingEnded", () => {
    endings += 1;
  });
  const ended = onceEnded(mixer);
  await startLive(engine, mixer);

  // The 3 s fade is clipped to the second there is, so the stems fall from frame 0 to 48000:
  // half way at frame 24000, which is sample 64 of block 188, and silent from block 376 on
  engine.pump(187 - FADE_BLOCKS);
  expect(left(engine.pump(1))[64]).toBeCloseTo(0.25, 3);
  engine.pump(SECOND_BLOCKS - 188 - 1);
  const last = left(engine.pump(1));
  expect(last.at(-1)).toBeCloseTo(0, 3);
  expect(engine.pump(10).every((sample) => sample === 0)).toBe(true);
  await ended;

  engine.pump(SECOND_BLOCKS);
  const settled = mixer.seek(0);
  engine.pump(1);
  await settled;
  expect(endings).toBe(1);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a live key change moves a tone by the asked semitones once its crossfade is done", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backing(harmonicTone(220, 3)) });
  await rendered(mixer);
  await startLive(engine, mixer);
  engine.pump(HALF_SECOND_BLOCKS / 2);

  const keyed = mixer.setBackingKey(2);
  engine.pump(1);
  await keyed;
  engine.pump(HALF_SECOND_BLOCKS);
  const heard = left(engine.pump(SECOND_BLOCKS));
  const expected = 220 * 2 ** (2 / 12);
  expect(Math.abs(cents(medianPitch(heard, expected), expected))).toBeLessThan(5);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a render that fails rejects start with the engine's message", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [],
    backing: {
      stems: [{ kind: "instrumental", path: join(dir, "missing.wav") }],
      key: 0,
      speed: 1,
      stemGains: unity,
    },
  });
  await expect(mixer.start()).rejects.toThrow(/missing\.wav/);
  await mixer.destroy();
  await engine.close();
});

test("closing the engine while a render runs rejects the start waiting on it", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backing(sine(440, 3), { key: 2 }) });
  const started = mixer.start();
  await engine.close();
  await expect(started).rejects.toThrow(/closed/);
});

test("the backing calls are refused without a backing or before it is live, and ranges are checked", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const bare = await engine.createMixer({ slots: [{ kind: "push" }] });
  await expect(bare.seek(0)).rejects.toThrow(/no backing/);
  await expect(bare.pause()).rejects.toThrow(/no backing/);
  await expect(bare.resume()).rejects.toThrow(/no backing/);
  await expect(bare.setBackingKey(1)).rejects.toThrow(/no backing/);
  expect(() => bare.setStemGain("instrumental", 1)).toThrow(/no backing/);

  const armed = await engine.createMixer({ slots: [], backing: backing(constant(0.5, 1)) });
  await expect(armed.seek(0)).rejects.toThrow(/armed/);
  await expect(armed.setBackingKey(13)).rejects.toThrow(RangeError);
  expect(() => armed.setStemGain("guideVocals", 5)).toThrow(RangeError);

  const stems = backing(constant(0.5, 1)).stems;
  const create = (config: Partial<BackingConfig>) =>
    engine.createMixer({
      slots: [],
      backing: { stems, key: 0, speed: 1, stemGains: unity, ...config },
    });
  await expect(create({ stems: [] })).rejects.toThrow(/instrumental stem/);
  await expect(create({ stems: [...stems, ...stems] })).rejects.toThrow(/two instrumental/);
  await expect(create({ stems: [{ kind: "instrumental", path: "" }] })).rejects.toThrow(TypeError);
  await expect(create({ key: 13 })).rejects.toThrow(RangeError);
  await expect(create({ key: 0.5 })).rejects.toThrow(TypeError);
  await expect(create({ speed: 0 })).rejects.toThrow(RangeError);
  await expect(create({ stemGains: { instrumental: -1, guideVocals: 1 } })).rejects.toThrow(
    RangeError,
  );
  await expect(create({ stopAtMs: -1 })).rejects.toThrow(RangeError);
  await engine.close();
});
