import { expect, test } from "vite-plus/test";
import { openEngine } from "../src/index.ts";

const BLOCK = 128;

test("an offline engine pumps silent blocks of 128 stereo frames at 48 kHz", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(engine.output).toEqual({ sampleRate: 48000, periodFrames: 128, latencyMs: 0 });
  expect(engine.capabilities).toEqual(new Set(["level"]));
  expect(engine.slots).toBe(8);

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
    engine.reactions.play(new Float32Array(BLOCK).fill(0.1), 1);
  }
  engine.reactions.play(new Float32Array(BLOCK).fill(1), 1);
  const sum = engine.pump(1);
  expect(sum.every((sample) => Math.abs(sample - 0.4) < 1e-6)).toBe(true);
  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);

  const stereo = new Float32Array(BLOCK * 2);
  for (let frame = 0; frame < BLOCK; frame += 1) {
    stereo[2 * frame] = 0.5;
    stereo[2 * frame + 1] = -0.5;
  }
  engine.reactions.play(stereo, 2);
  const played = engine.pump(1);
  for (let frame = 0; frame < BLOCK; frame += 1) {
    expect(played[2 * frame]).toBeCloseTo(0.5, 6);
    expect(played[2 * frame + 1]).toBeCloseTo(-0.5, 6);
  }
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
