// Run by bundle.test.ts in a process of its own: a song with a session bundle whose process dies
// mid-song, with no `destroy` and no exit handler.
import { openEngine } from "../../src/index.ts";

const dir = process.argv[2];
if (dir === undefined) throw new Error("usage: bundle-dies DIR");

const engine = await openEngine({ output: { offline: true } });
const mixer = await engine.createMixer({
  slots: [{ kind: "rtp" }, { kind: "push" }],
  bundle: { dir },
});
const started = mixer.start();
engine.pump(3);
await started;
mixer.setSlotEffects(1, [{ kind: "reverb", enabled: true }]);
engine.pump(8);
mixer.setSlot(0, { gain: 0.5 });
process.kill(process.pid, "SIGKILL");
