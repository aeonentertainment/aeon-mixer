import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type BackingStem, type OutputInfo, TRANSPORTS } from "../types.ts";
import { messageOf, SETTLE_MS, startLive, within } from "./offline-clock.ts";
import type { ContractMixer, ContractPromise, Trial } from "./contract-subject.ts";

const RATE = 48000;

/** A quarter second of a 440 Hz sine as a mono 16-bit WAV at 48 kHz. */
function sineWav() {
  const frames = RATE / 4;
  const wav = Buffer.alloc(44 + frames * 2);
  wav.write("RIFF", 0);
  wav.writeUInt32LE(36 + frames * 2, 4);
  wav.write("WAVEfmt ", 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(RATE, 24);
  wav.writeUInt32LE(RATE * 2, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write("data", 36);
  wav.writeUInt32LE(frames * 2, 40);
  for (let i = 0; i < frames; i += 1) {
    wav.writeInt16LE(Math.round(16000 * Math.sin((2 * Math.PI * 440 * i) / RATE)), 44 + 2 * i);
  }
  return wav;
}

function rendered(mixer: ContractMixer) {
  return new Promise<void>((resolve) => {
    mixer.on("renderProgress", (fraction) => {
      if (fraction === 1) resolve();
    });
  });
}

/** The first 38 blocks, a tenth of a second, an offline engine plays of `stem` as its backing. */
async function playedBy(trial: Trial, stem: BackingStem) {
  const { engine } = await trial.open();
  const backing = { stems: [stem], key: 0, speed: 1, stemGains: { instrumental: 1 } };
  const mixer = await engine.createMixer({ slots: [], backing });
  await rendered(mixer);
  await startLive(engine, mixer);
  return engine.pump(38);
}

export const enginePromises: ReadonlyArray<ContractPromise> = [
  {
    promise:
      "`outputDevices()` lists each device with an id, a name, whether it is the default, and one of the `Transport` names.",
    async check(trial) {
      const devices = await trial.outputDevices();
      if (devices.length === 0) return { unchecked: "the OS here lists no outputs" };
      const odd = devices.find(
        ({ id, name, isDefault, transport }) =>
          typeof id !== "string" ||
          typeof name !== "string" ||
          typeof isDefault !== "boolean" ||
          !new Set<string>(TRANSPORTS).has(transport),
      );
      return odd && { found: `it listed ${JSON.stringify(odd)}` };
    },
  },
  {
    promise: "A stem handed in as bytes plays the samples the same file plays by path.",
    async check(trial) {
      const wav = sineWav();
      const dir = await mkdtemp(join(tmpdir(), "aeon-contract-"));
      try {
        const path = join(dir, "stem.wav");
        await writeFile(path, wav);
        const byPath = await playedBy(trial, { kind: "instrumental", path });
        const byBytes = await playedBy(trial, { kind: "instrumental", bytes: new Uint8Array(wav) });
        const heard = byPath.some((sample) => Math.abs(sample) > 0.1);
        const same = byPath.length === byBytes.length && byPath.every((s, i) => s === byBytes[i]);
        if (heard && same) return undefined;
        return {
          found: heard
            ? "the bytes played other samples than the path"
            : "the stem by path played silence",
        };
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    },
  },
  {
    promise: "`destroy` settles when the engine closes in the middle of its swap.",
    async check(trial) {
      const { engine } = await trial.open();
      const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
      await startLive(engine, mixer);
      // Heard from the moment it is made, since the paths that return before reading it still
      // leave it to settle; its deadline covers the close's as well as its own
      const destroyed = within(mixer.destroy(), 2 * SETTLE_MS);
      // One block takes the swap; the fade out runs into the next, which never comes
      engine.pump(1);
      const closed = await within(engine.close(), SETTLE_MS);
      if (closed === "pending") return { found: "close never settled with a destroy in flight" };
      if ("error" in closed) return { found: `close rejected: ${messageOf(closed.error)}` };
      if ((await destroyed) !== "pending") return undefined;
      return { found: "destroy never settled after the engine closed mid-swap" };
    },
  },
  {
    promise:
      "After a move, `outputChanged` carries `engine.output`, the gap, and `heldMs`, which is 0 with no song playing.",
    async check(trial) {
      const { engine, move } = await trial.open();
      if (!move) {
        return {
          unchecked: "an offline engine never moves; give `moveOutput` to check a stand-in",
        };
      }
      const changed = new Promise<{ output: OutputInfo; gapMs: number; heldMs: number }>(
        (resolve) => {
          engine.on("outputChanged", (output, gapMs, heldMs) => resolve({ output, gapMs, heldMs }));
        },
      );
      move();
      const heard = await within(changed, SETTLE_MS);
      if (heard === "pending" || "error" in heard)
        return { found: "no outputChanged followed the move" };
      const { output, gapMs, heldMs } = heard.value;
      if (JSON.stringify(output) !== JSON.stringify(engine.output)) {
        return { found: "the output it carried is not the one `engine.output` names" };
      }
      if (!Number.isFinite(gapMs) || gapMs < 0) return { found: `its gapMs was ${gapMs}` };
      if (heldMs !== 0) return { found: `its heldMs was ${heldMs} with no song playing` };
      return undefined;
    },
  },
];
