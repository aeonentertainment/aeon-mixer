import { defaultProfile } from "../profile/default-profile.ts";
import { BLOCK, startLive } from "./offline-clock.ts";
import type {
  ContractEngine,
  ContractMixer,
  ContractPromise,
  Trial,
  Verdict,
} from "./contract-subject.ts";

const LEVEL = 0.5;

/**
 * The gain one pushed block plays at on a slot panned hard left, where the pan law passes it
 * whole; NaN when the block does not play at one level.
 */
function gainHeard(engine: ContractEngine, mixer: ContractMixer) {
  mixer.push(0, new Float32Array(2 * BLOCK).fill(LEVEL));
  const left = engine.pump(1).filter((_, i) => i % 2 === 0);
  const first = left[0] ?? Number.NaN;
  return left.every((sample) => Math.abs(sample - first) < 1e-5) ? first / LEVEL : Number.NaN;
}

const near = (heard: number, wanted: number) => Math.abs(heard - wanted) < 1e-4;

/** What an idle rtp, l16 and push slot report, under a delay `pinned` or left to choose. */
async function idleStats(trial: Trial, pinned: number | undefined): Promise<Verdict> {
  const { engine } = await trial.open(pinned === undefined ? {} : { playoutDelayMs: pinned });
  const mixer = await engine.createMixer({
    slots: [{ kind: "rtp" }, { kind: "l16" }, { kind: "push" }],
  });
  const stats = mixer.stats().slots;
  await mixer.destroy();
  const opening = pinned ?? 15;
  const wanted = [opening, opening, 0];
  const pin = pinned === undefined ? "choosing its delay" : `pinned at ${pinned}`;
  for (const [i, kind] of ["rtp", "l16", "push"].entries()) {
    const { late, lateAtOpening, playoutDelayMs } = stats[i] ?? {};
    if (late !== 0 || lateAtOpening !== 0 || playoutDelayMs !== wanted[i]) {
      return {
        found: `an idle ${kind} slot ${pin} reported late ${late}, lateAtOpening ${lateAtOpening} and playoutDelayMs ${playoutDelayMs}`,
      };
    }
  }
  return undefined;
}

export const slotPromises: ReadonlyArray<ContractPromise> = [
  {
    promise: "A slot nobody has set plays at the profile's `slot.gain`.",
    async check(trial) {
      const gain = 0.7;
      const { engine } = await trial.open({
        profile: { ...defaultProfile, slot: { ...defaultProfile.slot, gain } },
      });
      const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
      mixer.setSlot(0, { pan: -1 });
      await startLive(engine, mixer);
      const heard = gainHeard(engine, mixer);
      if (near(heard, gain)) return undefined;
      return {
        found: `a slot nobody set played at gain ${heard.toFixed(3)} under a profile of ${gain}`,
      };
    },
  },
  {
    promise: "A gain set while the mixer is armed is the gain its slot starts at.",
    async check(trial) {
      const { engine } = await trial.open();
      const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
      mixer.setSlot(0, { gain: 0.5, pan: -1 });
      await startLive(engine, mixer);
      const heard = gainHeard(engine, mixer);
      if (near(heard, 0.5)) return undefined;
      return { found: `a slot set to gain 0.5 while armed started at ${heard.toFixed(3)}` };
    },
  },
  {
    promise:
      "A slot that has heard nothing reports `late` and `lateAtOpening` 0, and `playoutDelayMs` where it opens: 15 when it chooses, the pinned delay when pinned, 0 for a `push` slot.",
    async check(trial) {
      const found = await Promise.all([undefined, 20].map((pinned) => idleStats(trial, pinned)));
      return found.find((verdict) => verdict !== undefined);
    },
  },
  {
    promise: "A slot's `tone` is null while its tone stage is off, and reports once it is on.",
    async check(trial) {
      const { engine } = await trial.open();
      const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
      await startLive(engine, mixer);
      const tone = () => mixer.stats().slots[0]?.tone;
      const off = tone();
      mixer.setSlotEffects(0, [{ kind: "tone", enabled: true, plosiveDb: 12, deEssDb: 6 }]);
      engine.pump(2);
      const on = tone();
      const reports =
        typeof on?.deEssDb === "number" && typeof on.plosives === "number" && on.match === null;
      if (off === null && reports) return undefined;
      return {
        found: `tone read ${JSON.stringify(off)} with the stage off and ${JSON.stringify(on)} with it on`,
      };
    },
  },
];
