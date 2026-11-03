import { createSocket } from "node:dgram";
import { once } from "node:events";
import { expect, test } from "vite-plus/test";
import { type Mixer, type OfflineEngine, openEngine } from "../src/index.ts";

const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
const SILENT_STATS = { pushed: 0, lost: 0, late: 0, concealed: 0, resyncs: 0, jitterMs: 0 };

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

/** Binds a second UDP socket to `port` the way a sender's neighbour would, and reports the error. */
async function bindAgain(port: number) {
  const socket = createSocket("udp4");
  const failed = once(socket, "error").then(([error]: unknown[]) => error);
  socket.bind(port);
  const outcome = await Promise.race([failed, once(socket, "listening").then(() => null)]);
  socket.close();
  return outcome;
}

test("rtp and l16 slots listen on their own ports, push slots on none", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [{ kind: "rtp" }, { kind: "l16" }, { kind: "push" }],
  });
  const kinds = mixer.slots.map((slot) => slot.kind);
  expect(kinds).toEqual(["rtp", "l16", "push"]);
  expect(mixer.slots.map((slot) => slot.index)).toEqual([0, 1, 2]);
  const ports = mixer.slots.slice(0, 2).map((slot) => slot.port);
  for (const port of ports) {
    expect(Number.isInteger(port)).toBe(true);
    expect(port).toBeGreaterThan(0);
    expect(port).toBeLessThan(65536);
  }
  expect(new Set(ports).size).toBe(2);
  expect(mixer.slots[2]?.port).toBeNull();

  // The next song's mixer is armed while this one exists, on ports of its own
  const next = await engine.createMixer({ slots: [{ kind: "rtp" }] });
  expect(ports).not.toContain(next.slots[0]?.port);
  await next.destroy();

  const held = await bindAgain(ports[0] ?? 0);
  expect(held).toMatchObject({ code: "EADDRINUSE" });
  await destroyLive(engine, mixer);
  await engine.close();
});

test("a mixed mixer runs silent until packets arrive, reports every slot's counters, and resets", async () => {
  const engine = await openEngine({ output: { offline: true }, playoutDelayMs: 20 });
  const mixer = await engine.createMixer({
    slots: [{ kind: "rtp" }, { kind: "push" }, { kind: "l16" }],
  });
  mixer.setSlot(0, { gain: 2, pan: 1 });
  mixer.setSlotEffects(2, [{ kind: "reverb", enabled: true, roomSize: 0.5, mix: 0.3 }]);
  expect(mixer.stats()).toEqual({
    slots: [SILENT_STATS, SILENT_STATS, SILENT_STATS],
    underruns: 0,
  });
  await startLive(engine, mixer);
  expect(engine.pump(10).every((sample) => sample === 0)).toBe(true);

  mixer.push(1, new Float32Array(2 * BLOCK).fill(0.5));
  expect(engine.pump(1).some((sample) => sample !== 0)).toBe(true);
  expect(() => mixer.push(0, new Float32Array(2 * BLOCK))).toThrow(/only a push slot/);
  expect(() => mixer.push(2, new Float32Array(2 * BLOCK))).toThrow(/only a push slot/);

  mixer.resetSlot(0);
  mixer.resetSlot(2);
  expect(engine.pump(1).every((sample) => sample === 0)).toBe(true);
  expect(mixer.stats().slots).toEqual([SILENT_STATS, SILENT_STATS, SILENT_STATS]);
  await destroyLive(engine, mixer);
  await engine.close();
});

test("the slot count and the playout delay are checked before anything crosses", async () => {
  await expect(openEngine({ output: { offline: true }, playoutDelayMs: 4 })).rejects.toThrow(
    RangeError,
  );
  await expect(openEngine({ output: { offline: true }, playoutDelayMs: 101 })).rejects.toThrow(
    RangeError,
  );
  const engine = await openEngine({ output: { offline: true }, slots: 2 });
  await expect(
    engine.createMixer({ slots: [{ kind: "rtp" }, { kind: "l16" }, { kind: "rtp" }] }),
  ).rejects.toThrow(RangeError);
  await engine.close();
});
