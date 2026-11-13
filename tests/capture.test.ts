import { expect, test } from "vite-plus/test";
import { openEngine } from "../src/index.ts";

// A capture slot opens a real input, and no test opens a device (docs/design.md section 9). Its
// ring and drift correction are proven in Rust on the offline driver; here, what reaches the addon.

test("an offline engine lists no inputs and refuses a capture slot, since it opens no device", async () => {
  const engine = await openEngine({ output: { offline: true } });
  expect(await engine.inputDevices()).toEqual([]);
  await expect(
    engine.createMixer({ slots: [{ kind: "push" }, { kind: "capture", device: "a mic" }] }),
  ).rejects.toThrow(/offline engine opens no input/);
  await expect(engine.createMixer({ slots: [{ kind: "capture" }] })).rejects.toThrow(
    /offline engine/,
  );
  await engine.close();
  await expect(engine.inputDevices()).rejects.toThrow(/closed/);
});

test("a capture slot's device and channel are checked before anything crosses", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const create = (slot: Parameters<typeof engine.createMixer>[0]["slots"][number]) =>
    engine.createMixer({ slots: [slot] });
  await expect(create({ kind: "capture", channel: -1 })).rejects.toThrow(RangeError);
  await expect(create({ kind: "capture", channel: 256 })).rejects.toThrow(RangeError);
  await expect(create({ kind: "capture", channel: 1.5 })).rejects.toThrow(TypeError);
  await expect(create({ kind: "capture", device: "" })).rejects.toThrow(/inputDevices/);
  await engine.close();
});

// A capture slot's input opens with the native mixer, and a mixer that never comes back cannot
// release it. So the backing is checked first: on an offline engine, which refuses a capture slot
// only as the native mixer opens, bad backing rejects with its own error and never reaches that
// point; good backing reaches it.
test("bad backing is refused before a capture slot's input could open", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const slots = [{ kind: "capture" as const, device: "a mic" }];
  const backing = {
    stems: [{ kind: "guideVocals" as const, path: "vocals.wav" }],
    key: 0,
    speed: 1,
    stemGains: { instrumental: 1, guideVocals: 1 },
  };
  await expect(engine.createMixer({ slots, backing })).rejects.toThrow(
    /needs an instrumental stem/,
  );
  await expect(
    engine.createMixer({
      slots,
      backing: { ...backing, stemGains: { instrumental: 9, guideVocals: 1 } },
    }),
  ).rejects.toThrow(RangeError);
  const good = { ...backing, stems: [{ kind: "instrumental" as const, path: "song.wav" }] };
  await expect(engine.createMixer({ slots, backing: good })).rejects.toThrow(
    /offline engine opens no input/,
  );
  await engine.close();
});

// A mixer's capture inputs are its native mixer's to hold. Closing the engine releases every mixer
// it made, live or armed, whatever state the engine is in. A capture slot cannot open offline, so
// push slots show it: a released mixer refuses pushed audio.
test("closing the engine releases every mixer, armed or live", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const live = await engine.createMixer({ slots: [{ kind: "push" }] });
  const started = live.start();
  engine.pump(3);
  await started;
  const armed = await engine.createMixer({ slots: [{ kind: "push" }] });
  expect(armed.push(0, new Float32Array(256))).toBe(128);
  // The live mixer's swap out waits on the offline clock like any command
  const closed = engine.close();
  engine.pump(3);
  await closed;
  expect(() => armed.push(0, new Float32Array(256))).toThrow(/destroyed/);
  expect(() => live.push(0, new Float32Array(256))).toThrow(/destroyed/);
});
