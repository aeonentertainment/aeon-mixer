import { expect, test, vi } from "vite-plus/test";
import { type Mixer, type OfflineEngine, openEngine, type SlotEffect } from "../src/index.ts";

const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
const CENTRE = Math.SQRT1_2;

/** Interleaved stereo at one level, which folds to that level. */
function constant(level: number, blocks = 1) {
  return new Float32Array(2 * BLOCK * blocks).fill(level);
}

function left(interleaved: Float32Array): number[] {
  return Array.from(interleaved.filter((_, i) => i % 2 === 0));
}

function right(interleaved: Float32Array): number[] {
  return Array.from(interleaved.filter((_, i) => i % 2 === 1));
}

function firstSound(samples: number[]) {
  return samples.findIndex((sample) => sample !== 0);
}

function onceLevel(mixer: Mixer): Promise<[slot: number, rmsDb: number, peakDb: number]> {
  return new Promise((resolve) => {
    const off = mixer.on("level", (slot, rmsDb, peakDb) => {
      off();
      resolve([slot, rmsDb, peakDb]);
    });
  });
}

/** Starts the mixer, pumping the offline clock through the swap fade so the acks arrive. */
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

test("a push slot plays what the host pushes, through its gain and the pan law", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }, { kind: "push" }] });
  expect(mixer.slots).toEqual([
    { index: 0, kind: "push", port: null },
    { index: 1, kind: "push", port: null },
  ]);
  mixer.setSlot(0, { gain: 0.5, pan: -1 });
  mixer.setSlot(1, { gain: 0.5 });
  await startLive(engine, mixer);

  expect(mixer.push(0, constant(0.5))).toBe(BLOCK);
  expect(mixer.push(1, constant(0.5))).toBe(BLOCK);
  const out = engine.pump(1);
  for (const sample of left(out)) expect(sample).toBeCloseTo(0.25 + 0.25 * CENTRE, 5);
  for (const sample of right(out)) expect(sample).toBeCloseTo(0.25 * CENTRE, 5);

  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);
  // A slot's ring holds 100 ms; the rest is the host's to hand in again, and the count says so
  expect(mixer.push(1, constant(1, 400))).toBe(4800);
  expect(mixer.stats()).toEqual({
    slots: [
      {
        pushed: 0,
        lost: 0,
        late: 0,
        concealed: 0,
        resyncs: 0,
        jitterMs: 0,
        correctorDelayMs: 0,
        correctorMaxDelayMs: 0,
        echo: null,
      },
      {
        pushed: 0,
        lost: 0,
        late: 0,
        concealed: 0,
        resyncs: 0,
        jitterMs: 0,
        correctorDelayMs: 0,
        correctorMaxDelayMs: 0,
        echo: null,
      },
    ],
    underruns: 0,
    rtpReturn: null,
  });
  await destroyLive(engine, mixer);
  await engine.close();
});

test("start resolves on the acks, reports the output latency, and refuses a second live mixer", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  const before = Date.now();
  const started = await startLive(engine, mixer);
  expect(started.startedAt).toBeGreaterThanOrEqual(before);
  expect(started.startedAt).toBeLessThanOrEqual(Date.now() + engine.output.latencyMs);
  expect(started.backingOffsetMs).toBe(engine.output.latencyMs);
  expect(started.speedFactor).toBe(1);

  await expect(mixer.start()).rejects.toThrow(/cannot start: it is live/);
  const next = await engine.createMixer({ slots: [{ kind: "push" }] });
  await expect(next.start()).rejects.toThrow(/another mixer is live/);
  await destroyLive(engine, mixer);
  await startLive(engine, next);
  await destroyLive(engine, next);
  await engine.close();
});

test("settings on an armed mixer wait for its start and never touch the live one", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const live = await engine.createMixer({ slots: [{ kind: "push" }] });
  await startLive(engine, live);
  const armed = await engine.createMixer({ slots: [{ kind: "push" }] });
  armed.setSlot(0, { mute: true });
  armed.setSlotEffects(0, [{ kind: "reverb", enabled: false, roomSize: 0.5, mix: 0.2 }]);

  live.push(0, constant(0.5));
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.5 * CENTRE) < 1e-5)).toBe(true);

  await destroyLive(engine, live);
  await startLive(engine, armed);
  armed.push(0, constant(0.5));
  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);
  armed.setSlot(0, { mute: false });
  armed.push(0, constant(0.5, 2));
  engine.pump(1);
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.5 * CENTRE) < 1e-5)).toBe(true);

  // Enabled live, the reverb is in the chain and the slot no longer plays the dry level
  armed.setSlotEffects(0, [{ kind: "reverb", enabled: true, roomSize: 0.5, mix: 0.2 }]);
  armed.push(0, constant(0.5, 4));
  engine.pump(3);
  expect(left(engine.pump(1)).some((sample) => Math.abs(sample - 0.5 * CENTRE) > 1e-3)).toBe(true);
  await destroyLive(engine, armed);
  await engine.close();
});

/**
 * Plays a room nobody sings over, white noise at -50 dBFS RMS, through one slot with `effects`
 * on, and gives the last half second's level in dBFS at the slot, once a gate would have learned
 * the room and closed.
 */
async function roomTailDb(effects: SlotEffect[]) {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  mixer.setSlotEffects(0, effects);
  await startLive(engine, mixer);
  // A host pushing a block ahead of the clock
  let seed = 1;
  const amplitude = 10 ** (-50 / 20) * Math.sqrt(3);
  const roomBlock = () => {
    const block = new Float32Array(2 * BLOCK);
    for (let i = 0; i < BLOCK; i += 1) {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      block[2 * i] = block[2 * i + 1] = amplitude * (seed / 2 ** 31 - 1);
    }
    return block;
  };
  const heard: number[] = [];
  for (let i = 0; i < 750; i += 1) {
    mixer.push(0, roomBlock());
    heard.push(...left(engine.pump(1)));
  }
  const tail = heard.slice(-24000);
  const rmsDb = 10 * Math.log10(tail.reduce((sum, s) => sum + s * s, 0) / tail.length);
  await destroyLive(engine, mixer);
  await engine.close();
  return rmsDb - 20 * Math.log10(CENTRE);
}

test("cleanup turns a room nobody sings over down by the gate's 18 dB less its 6 dB of makeup", async () => {
  expect(await roomTailDb([{ kind: "cleanup", enabled: true }])).toBeCloseTo(-50 - 12, 0);
});

// The canceller has no echo to take out here, so a room it passes keeps its level; cleanup, had
// it come on with the canceller, would gate it 12 dB down
test("turning on the echo canceller leaves cleanup off", async () => {
  expect(await roomTailDb([{ kind: "echo-cancel", enabled: true }])).toBeCloseTo(-50, 0);
});

test("level events read the slot's RMS and peak within a tenth of a decibel", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  // A host pushes as it captures, a block ahead of the clock; the meter sits before gain, so it
  // reads the sine through the swap fade too
  let frame = 0;
  const sineBlock = () => {
    const block = new Float32Array(2 * BLOCK);
    for (let i = 0; i < BLOCK; i += 1, frame += 1) {
      const sample = 0.5 * Math.sin((2 * Math.PI * 1000 * frame) / 48000);
      block[2 * i] = sample;
      block[2 * i + 1] = sample;
    }
    return block;
  };
  const level = onceLevel(mixer);
  for (let i = 0; i < FADE_BLOCKS; i += 1) mixer.push(0, sineBlock());
  await startLive(engine, mixer);
  const blocks = Math.ceil(4800 / BLOCK);
  for (let i = FADE_BLOCKS; i < blocks; i += 1) {
    mixer.push(0, sineBlock());
    engine.pump(1);
  }
  const [slot, rmsDb, peakDb] = await level;
  expect(slot).toBe(0);
  expect(rmsDb).toBeCloseTo(-9.03, 1);
  expect(peakDb).toBeCloseTo(-6.02, 1);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("pitch events read a sung tone's frequency on the slot that sang it", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }, { kind: "push" }] });
  let frame = 0;
  const sineBlock = () => {
    const block = new Float32Array(2 * BLOCK);
    for (let i = 0; i < BLOCK; i += 1, frame += 1) {
      const sample = 0.5 * Math.sin((2 * Math.PI * 220 * frame) / 48000);
      block[2 * i] = sample;
      block[2 * i + 1] = sample;
    }
    return block;
  };
  // The detector reads every 10 ms; the first windows still hold the silence before the tone
  const reading = new Promise<[slot: number, hz: number]>((resolve) => {
    const off = mixer.on("pitch", (slot, _positionMs, hz, confidence) => {
      if (confidence < 0.9) return;
      off();
      resolve([slot, hz]);
    });
  });
  await startLive(engine, mixer);
  for (let i = 0; i < Math.ceil(9600 / BLOCK); i += 1) {
    mixer.push(1, sineBlock());
    engine.pump(1);
  }
  const [slot, hz] = await reading;
  expect(slot).toBe(1);
  // Within 10 cents of A3, the corrector tests' tolerance; a pure sine reads about 6 cents off
  expect(Math.abs(1200 * Math.log2(hz / 220))).toBeLessThan(10);
  await destroyLive(engine, mixer);
  await engine.close();
});

/**
 * The first voiced reading of a slot singing 30 cents above A3, 0.7 of a semitone under the
 * guide's B flat 3, with correction `enabled` or not.
 */
async function firstVoicedReading(enabled: boolean) {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  // With no backing the guide's clock holds still; the offset parks it inside the B flat
  mixer.setSlotGuide(0, { notes: [{ midi: 58, startMs: 0, endMs: 1000 }], keyMask: 0 }, 500);
  mixer.setSlotEffects(0, [{ kind: "pitch-correction", enabled, strength: 1, retuneMs: 0 }]);
  const hz = 220 * 2 ** (30 / 1200);
  let frame = 0;
  // Six harmonics, so the detector reads it as it reads a voice
  const voiceBlock = () => {
    const block = new Float32Array(2 * BLOCK);
    for (let i = 0; i < BLOCK; i += 1, frame += 1) {
      let sample = 0;
      for (let h = 1; h <= 6; h += 1)
        sample += Math.sin((2 * Math.PI * hz * h * frame) / 48000) / h;
      block[2 * i] = 0.25 * sample;
      block[2 * i + 1] = 0.25 * sample;
    }
    return block;
  };
  const reading = new Promise<[targetMidi: number | null, errorSemitones: number]>((resolve) => {
    const off = mixer.on("pitch", (_slot, _positionMs, _hz, confidence, targetMidi, error) => {
      if (confidence < 0.9) return;
      off();
      resolve([targetMidi, error]);
    });
  });
  await startLive(engine, mixer);
  for (let i = 0; i < Math.ceil(9600 / BLOCK); i += 1) {
    mixer.push(0, voiceBlock());
    engine.pump(1);
  }
  const shown = await reading;
  await destroyLive(engine, mixer);
  await engine.close();
  return shown;
}

test("a corrected slot's pitch events name the guide note and how far flat the voice is", async () => {
  const [targetMidi, errorSemitones] = await firstVoicedReading(true);
  expect(targetMidi).toBe(58);
  // 10 cents, the corrector tests' tolerance for the detector's error
  expect(Math.abs(errorSemitones - 0.7)).toBeLessThan(0.1);
});

test("with correction off a pitch event names no target and no error", async () => {
  expect(await firstVoicedReading(false)).toEqual([null, 0]);
});

test("destroy swaps silence in, resolves once the graph is back, and the mixer refuses after", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  await startLive(engine, mixer);
  mixer.push(0, constant(1, 8));
  expect(engine.pump(1).some((sample) => sample !== 0)).toBe(true);

  await destroyLive(engine, mixer);
  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);
  expect(() => mixer.push(0, constant(1))).toThrow(/destroyed/);
  await mixer.destroy();
  await engine.close();
});

test("resetSlot forgets what the host pushed, and a guide can be set and cleared live", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  mixer.setSlotGuide(0, {
    notes: [{ midi: 60, startMs: 0, endMs: 500 }],
    keyMask: 0b1010_1011_0101,
  });
  await startLive(engine, mixer);
  mixer.setSlotGuide(0, { notes: [{ midi: 62, startMs: 500, endMs: 1000 }], keyMask: 0b101 }, 20);
  mixer.setSlotGuide(0, null);

  mixer.push(0, constant(0.5, 3));
  mixer.resetSlot(0);
  expect(engine.pump(3).every((sample) => sample === 0)).toBe(true);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a push slot plays at most 30 ms behind the newest frame, dropping the oldest and counting it", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  mixer.setSlot(0, { pan: -1 });
  await startLive(engine, mixer);

  // 50 ms of one level, then exactly 30 ms of another: the first level is what gets dropped
  mixer.push(0, new Float32Array(2 * 2400).fill(0.25));
  mixer.push(0, new Float32Array(2 * 1440).fill(0.5));
  expect(left(engine.pump(1)).every((sample) => Math.abs(sample - 0.5) < 1e-6)).toBe(true);
  expect(mixer.stats().slots[0]?.resyncs).toBe(1);
  // Within the depth nothing more is dropped
  expect(left(engine.pump(10)).every((sample) => Math.abs(sample - 0.5) < 1e-6)).toBe(true);
  expect(mixer.stats().slots[0]?.resyncs).toBe(1);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a command the clock never serves rejects after a second with the engine's state", async () => {
  vi.useFakeTimers();
  try {
    const engine = await openEngine({ output: { offline: true } });
    const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
    const started = mixer.start();
    vi.advanceTimersByTime(1000);
    await expect(started).rejects.toThrow(/not acknowledged within 1000 ms; the engine is running/);
    const closing = engine.close();
    engine.pump(FADE_BLOCKS);
    await closing;
  } finally {
    vi.useRealTimers();
  }
});

test("ranges are checked before anything crosses", async () => {
  const engine = await openEngine({ output: { offline: true } });
  await expect(
    engine.createMixer({
      slots: [],
      backing: {
        stems: [],
        key: 0,
        speed: 1,
        stemGains: { instrumental: 1, guideVocals: 1 },
      },
    }),
  ).rejects.toThrow(/backing/);
  await expect(
    engine.createMixer({ slots: Array.from({ length: 9 }, () => ({ kind: "push" })) }),
  ).rejects.toThrow(RangeError);

  const mixer = await engine.createMixer({ slots: [{ kind: "push" }, { kind: "push" }] });
  expect(() => mixer.setSlot(2, { gain: 1 })).toThrow(RangeError);
  expect(() => mixer.setSlot(0, { gain: -1 })).toThrow(RangeError);
  expect(() => mixer.setSlot(0, { gain: 5 })).toThrow(RangeError);
  expect(() => mixer.setSlot(0, { pan: 2 })).toThrow(RangeError);
  expect(() => mixer.setSlot(0, { pan: Number.NaN })).toThrow(TypeError);
  expect(() =>
    mixer.setSlotEffects(0, [
      { kind: "pitch-correction", enabled: true, strength: 2, retuneMs: 0 },
    ]),
  ).toThrow(RangeError);
  expect(() =>
    mixer.setSlotEffects(0, [{ kind: "reverb", enabled: true, roomSize: 0.5, mix: -0.1 }]),
  ).toThrow(RangeError);
  expect(() =>
    mixer.setSlotEffects(0, [{ kind: "voice", enabled: true, formantSemitones: 13 }]),
  ).toThrow(RangeError);
  expect(() =>
    mixer.setSlotGuide(0, { notes: [{ midi: 128, startMs: 0, endMs: 1 }], keyMask: 0 }),
  ).toThrow(RangeError);
  expect(() =>
    mixer.setSlotGuide(0, { notes: [{ midi: 60, startMs: 10, endMs: 5 }], keyMask: 0 }),
  ).toThrow(RangeError);
  expect(() => mixer.setSlotGuide(0, { notes: [], keyMask: 4096 })).toThrow(RangeError);
  expect(() => mixer.setSlotGuide(0, null, 5000)).toThrow(RangeError);
  expect(() => mixer.push(0, new Float32Array(0))).toThrow(TypeError);
  expect(() => mixer.push(0, new Float32Array(3))).toThrow(RangeError);
  await expect(mixer.seek(1000)).rejects.toThrow(/backing/);
  await expect(mixer.setBackingKey(2)).rejects.toThrow(/backing/);
  expect(() => mixer.setStemGain("instrumental", 0.5)).toThrow(/backing/);
  await engine.close();
});

// A voice reaches the slot through the API: brighter changes the sound, and its first sample comes
// out on the same frame as the dry voice's, since a formant move adds no delay
test("a voice changes the sound and not when it plays", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const render = async (effects: Parameters<Mixer["setSlotEffects"]>[1]) => {
    const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
    mixer.setSlot(0, { pan: -1 });
    mixer.setSlotEffects(0, effects);
    await startLive(engine, mixer);
    let frame = 0;
    const out: number[] = [];
    for (let block = 0; block < 60; block += 1) {
      const pcm = new Float32Array(2 * BLOCK);
      for (let i = 0; i < BLOCK; i += 1, frame += 1) {
        let sample = 0;
        if (frame >= 1000) {
          const t = (frame - 1000) / 48000;
          for (let h = 1; h <= 12; h += 1) sample += Math.cos(2 * Math.PI * 147 * h * t) / h;
        }
        pcm[2 * i] = 0.2 * sample;
        pcm[2 * i + 1] = 0.2 * sample;
      }
      mixer.push(0, pcm);
      out.push(...left(engine.pump(1)));
    }
    await destroyLive(engine, mixer);
    return out;
  };
  const dry = await render([]);
  const brighter = await render([{ kind: "voice", enabled: true, formantSemitones: 2 }]);
  expect(firstSound(dry)).toBeGreaterThan(0);
  expect(firstSound(brighter)).toBe(firstSound(dry));
  expect(brighter.some((sample, i) => Math.abs(sample - (dry[i] ?? 0)) > 1e-3)).toBe(true);
  await engine.close();
});
