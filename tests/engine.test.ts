import { expect, test } from "vite-plus/test";
import { openEngine } from "../src/index.ts";

const BLOCK = 128;
/** One reading of the master meter, 100 ms at 48 kHz. */
const METER_WINDOW = 4800;
const LEVEL_TOLERANCE_DB = 0.01;

/** 1 kHz is 48 samples a cycle, so a meter window holds whole cycles and one exact crest. */
function sine(amplitude: number, frames: number) {
  return Float32Array.from(
    { length: frames },
    (_, i) => amplitude * Math.sin((2 * Math.PI * i) / 48),
  );
}

test("an offline engine pumps silent blocks of 128 stereo frames at 48 kHz", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(engine.output).toEqual({ sampleRate: 48000, periodFrames: 128, latencyMs: 0 });
  expect(engine.capabilities).toEqual(
    new Set(["guideVocal", "key", "speed", "seek", "shortVersion", "pause", "level"]),
  );

  const block = engine.pump(1);
  expect(block).toBeInstanceOf(Float32Array);
  expect(block).toHaveLength(BLOCK * 2);
  expect(block.every((sample) => sample === 0)).toBe(true);

  await engine.close();
});

test("the offline engine lists no outputs, so it never asks the OS", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(await engine.outputDevices()).toEqual([]);
  await engine.close();
});

test("a closed engine refuses everything, and closing twice is fine", async () => {
  const engine = await openEngine({ output: { offline: true } });
  await engine.close();
  await engine.close();
  expect(() => engine.pump(1)).toThrow(/closed/);
  await expect(engine.outputDevices()).rejects.toThrow(/closed/);
  await expect(engine.createMixer({ slots: [] })).rejects.toThrow(/closed/);
});

test("reactions play on both channels, four at once, and a fifth is dropped", async () => {
  const engine = await openEngine({ output: { offline: true } });
  for (let voice = 0; voice < 4; voice += 1) {
    expect(engine.reactions.play(new Float32Array(BLOCK).fill(0.1), 1)).toBe(true);
  }
  expect(engine.reactions.play(new Float32Array(BLOCK).fill(1), 1)).toBe(false);
  const sum = engine.pump(1);
  expect(sum.every((sample) => Math.abs(sample - 0.4) < 1e-6)).toBe(true);
  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);

  const stereo = new Float32Array(BLOCK * 2);
  for (let frame = 0; frame < BLOCK; frame += 1) {
    stereo[2 * frame] = 0.5;
    stereo[2 * frame + 1] = -0.5;
  }
  expect(engine.reactions.play(stereo, 2)).toBe(true);
  const played = engine.pump(1);
  for (let frame = 0; frame < BLOCK; frame += 1) {
    expect(played[2 * frame]).toBeCloseTo(0.5, 6);
    expect(played[2 * frame + 1]).toBeCloseTo(-0.5, 6);
  }
  await engine.close();
});

test("the engine reads the master level of what it plays, with no mixer live", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const reading = new Promise<[rmsDb: number, peakDb: number]>((resolve) => {
    const off = engine.on("masterLevel", (rmsDb, peakDb) => {
      off();
      resolve([rmsDb, peakDb]);
    });
  });
  engine.reactions.play(sine(0.5, METER_WINDOW), 1);
  engine.pump(Math.ceil(METER_WINDOW / BLOCK));

  // A half-scale sine, under the clipper's knee
  const [rmsDb, peakDb] = await reading;
  expect(Math.abs(rmsDb - -9.03)).toBeLessThan(LEVEL_TOLERANCE_DB);
  expect(Math.abs(peakDb - -6.02)).toBeLessThan(LEVEL_TOLERANCE_DB);
  await engine.close();
});

test("options and reaction buffers are checked before anything crosses", async () => {
  await expect(openEngine({ output: { offline: true }, slots: 17 })).rejects.toThrow(RangeError);
  await expect(openEngine({ output: { offline: true }, slots: 0 })).rejects.toThrow(RangeError);
  await expect(openEngine({ output: { offline: true }, playoutDelayMs: -1 })).rejects.toThrow(
    RangeError,
  );

  const engine = await openEngine({ output: { offline: true } });
  expect(() => engine.reactions.play(new Float32Array(3), 2)).toThrow(RangeError);
  expect(() => engine.reactions.play(new Float32Array(0), 1)).toThrow(TypeError);
  expect(() => engine.pump(0)).toThrow(RangeError);
  await engine.close();
});
