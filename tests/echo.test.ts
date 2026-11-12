import { expect, test } from "vite-plus/test";
import { type Mixer, type OfflineEngine, openEngine } from "../src/index.ts";

const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
/** The room in this test plays the mic the mix this many blocks after it was rendered. */
const PATH_BLOCKS = 10;

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

/** Repeatable noise in -1 to 1, the generator `aec:measure` uses. */
function noise(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return (state / 4294967296) * 2 - 1;
  };
}

/** A mono block as the interleaved stereo a push slot takes, the same on both sides. */
function stereo(mono: Float32Array) {
  const pcm = new Float32Array(2 * mono.length);
  mono.forEach((sample, i) => pcm.fill(sample, 2 * i, 2 * i + 2));
  return pcm;
}

function energy(samples: Iterable<number>) {
  let sum = 0;
  for (const sample of samples) sum += sample * sample;
  return sum;
}

// Two push slots: the far end, a noise bed panned right, and a mic panned left that hears the
// mix ten blocks after it was rendered, 10 dB down. Tolerances: the path within the finder's 16
// frames, and at least 20 dB out of the last second.
test("a slot's echo canceller finds the room's echo, takes it out, reports it, and forgets it when turned off", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }, { kind: "push" }] });
  mixer.setSlot(0, { pan: -1 });
  mixer.setSlot(1, { pan: 1 });
  mixer.setSlotEffects(0, [{ kind: "echo-cancel", enabled: true }]);
  expect(mixer.stats().slots[0]?.echo).toBeNull();
  await startLive(engine, mixer);

  const random = noise(7);
  const played: Float32Array[] = [];
  let heard = 0;
  let handedOn = 0;
  const blocks = 3 * 375;
  for (let i = 0; i < blocks; i++) {
    const far = new Float32Array(BLOCK).map(() => 0.1 * random());
    const echo =
      played[i - PATH_BLOCKS]?.map((sample) => 0.316 * sample) ?? new Float32Array(BLOCK);
    expect(mixer.push(1, stereo(far))).toBe(BLOCK);
    expect(mixer.push(0, stereo(echo))).toBe(BLOCK);
    const out = engine.pump(1);
    const left = out.filter((_, j) => j % 2 === 0);
    played.push(left.map((sample, j) => 0.5 * (sample + (out[2 * j + 1] ?? 0))));
    if (i >= blocks - 375) {
      heard += energy(echo);
      handedOn += energy(left);
    }
  }

  const echo = mixer.stats().slots[0]?.echo;
  expect(echo).not.toBeNull();
  expect(Math.abs((echo?.pathMs ?? 0) - (PATH_BLOCKS * BLOCK) / 48)).toBeLessThanOrEqual(16 / 48);
  expect(10 * Math.log10(heard / handedOn)).toBeGreaterThanOrEqual(20);
  expect(echo?.removedDb).toBeGreaterThanOrEqual(20);
  expect(mixer.stats().slots[0]?.howlCatches).toBe(0);

  mixer.setSlotEffects(0, []);
  engine.pump(1);
  expect(mixer.stats().slots[0]?.echo).toBeNull();
  await destroyLive(engine, mixer);
  await engine.close();
});
