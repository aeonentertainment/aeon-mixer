import { expect, test } from "vite-plus/test";
import { openEngine } from "../src/index.ts";

test("an offline engine pumps silent blocks of 128 stereo frames at 48 kHz", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(engine.output).toEqual({ sampleRate: 48000, periodFrames: 128, latencyMs: 0 });

  const block = engine.pump(1);
  expect(block).toBeInstanceOf(Float32Array);
  expect(block).toHaveLength(128 * 2);
  expect(block.every((sample) => sample === 0)).toBe(true);

  await engine.close();
});

test("a closed engine refuses to pump", async () => {
  const engine = await openEngine({ output: { offline: true } });
  await engine.close();
  expect(() => engine.pump(1)).toThrow(/closed/);
});
