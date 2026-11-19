import { expect, test } from "vite-plus/test";
import { openEngine, type ToneBand } from "../src/index.ts";

const lift = (hz: number): ToneBand => ({ kind: "peak", hz, gainDb: 6, q: 1 });
const presence = (gainDb: number): ToneBand => ({ kind: "peak", hz: 3000, gainDb, q: 1 });

const tone = (bands: ReadonlyArray<ToneBand>) => [{ kind: "tone" as const, enabled: true, bands }];

async function armed() {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  return { engine, mixer };
}

// The two caps at their edges, through the engine's own check on an armed mixer: a 6 dB lift of
// Q 1 at 1150 Hz is 0.199 ms late at worst and at 1120 Hz 0.205; two presence lifts that add to
// 5.95 dB are taken and to 6.05 refused
test("setSlotEffects takes tone bands just inside each cap and refuses them just outside by name", async () => {
  const { engine, mixer } = await armed();
  expect(() => mixer.setSlotEffects(0, tone([lift(1150)]))).not.toThrow();
  expect(() => mixer.setSlotEffects(0, tone([lift(1120)]))).toThrow(
    "the tone stage's bands and air leave 1078 Hz 0.205 ms late, over the 0.2 ms cap",
  );
  expect(() => mixer.setSlotEffects(0, tone([presence(3), presence(2.95)]))).not.toThrow();
  expect(() => mixer.setSlotEffects(0, tone([presence(3), presence(3.05)]))).toThrow(
    "the tone stage's bands and air lift 3000 Hz by 6.05 dB, over the 6 dB cap",
  );
  await mixer.destroy();
  await engine.close();
});

test("setSlotEffects refuses the blind match, which has no target yet, and a band out of range", async () => {
  const { engine, mixer } = await armed();
  expect(() => mixer.setSlotEffects(0, [{ kind: "tone", enabled: true, bands: "match" }])).toThrow(
    'bands "match" has no target curve to aim at yet',
  );
  expect(() => mixer.setSlotEffects(0, tone([{ kind: "low-shelf", hz: 79, gainDb: 1 }]))).toThrow(
    "bands[0].hz must be between 80 and 16000, got 79",
  );
  expect(() => mixer.setSlotEffects(0, tone([{ kind: "peak", hz: 900, gainDb: 1, q: 9 }]))).toThrow(
    "bands[0].q must be between 0.3 and 8, got 9",
  );
  const nine = Array.from({ length: 9 }, () => presence(0));
  expect(() => mixer.setSlotEffects(0, [{ kind: "tone", enabled: true, bands: nine }])).toThrow(
    "a tone stage takes at most 8 bands",
  );
  expect(() => mixer.setSlotEffects(0, [{ kind: "tone", enabled: true, airDb: 7 }])).toThrow(
    "airDb must be between 0 and 6, got 7",
  );
  await mixer.destroy();
  await engine.close();
});

// Live, stats reads the stage once it is on and null once it is off again
test("stats reports the tone stage while it is on", async () => {
  const { engine, mixer } = await armed();
  const started = mixer.start();
  engine.pump(3);
  await started;
  expect(mixer.stats().slots[0]?.tone).toBeNull();
  mixer.setSlotEffects(0, [{ kind: "tone", enabled: true, plosiveDb: 12, deEssDb: 6 }]);
  engine.pump(2);
  expect(mixer.stats().slots[0]?.tone).toEqual({ match: null, deEssDb: 0, plosives: 0 });
  mixer.setSlotEffects(0, []);
  engine.pump(2);
  expect(mixer.stats().slots[0]?.tone).toBeNull();
  const destroyed = mixer.destroy();
  engine.pump(3);
  await destroyed;
  await engine.close();
});
