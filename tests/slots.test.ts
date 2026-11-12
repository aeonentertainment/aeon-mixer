import { createSocket } from "node:dgram";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vite-plus/test";
import { type Mixer, type OfflineEngine, openEngine } from "../src/index.ts";

const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
const SILENT_STATS = {
  pushed: 0,
  lost: 0,
  late: 0,
  concealed: 0,
  resyncs: 0,
  jitterMs: 0,
  correctorDelayMs: 0,
  correctorMaxDelayMs: 0,
  echo: null,
};

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
  await mixer.destroy();
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
    rtpReturn: null,
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

test("a slot reports the corrector's delay while correction is on and nothing once it is off", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  const corrector = () => {
    const { correctorDelayMs, correctorMaxDelayMs } = mixer.stats().slots[0]!;
    return { correctorDelayMs, correctorMaxDelayMs };
  };
  await startLive(engine, mixer);
  engine.pump(1);
  expect(corrector()).toEqual({ correctorDelayMs: 0, correctorMaxDelayMs: 0 });

  mixer.setSlotEffects(0, [{ kind: "pitch-correction", enabled: true, strength: 1, retuneMs: 0 }]);
  engine.pump(1);
  const on = corrector();
  // With no voice yet the shifter rests on its floor, 59 frames, and the ceiling is 15 ms
  expect(on.correctorDelayMs).toBeCloseTo(59 / 48, 1);
  expect(on.correctorMaxDelayMs).toBe(15);

  mixer.setSlotEffects(0, []);
  // Turning off crossfades over 20 ms, eight blocks
  engine.pump(8);
  expect(corrector()).toEqual({ correctorDelayMs: 0, correctorMaxDelayMs: 0 });
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

test("an rtp slot records to a new pcap file, and a push slot or an existing file refuses", async () => {
  const dir = await mkdtemp(join(tmpdir(), "aeon-mixer-record-"));
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "rtp" }, { kind: "push" }] });
  await startLive(engine, mixer);

  const path = join(dir, "slot0.pcap");
  mixer.recordSlot(0, path);
  // Classic pcap: the microsecond magic little endian, version 2.4, Ethernet
  const header = await readFile(path);
  expect([...header.subarray(0, 4)]).toEqual([0xd4, 0xc3, 0xb2, 0xa1]);
  expect(header.readUInt16LE(4)).toBe(2);
  expect(header.readUInt32LE(20)).toBe(1);
  mixer.recordSlot(0, null);

  expect(() => {
    mixer.recordSlot(1, join(dir, "push.pcap"));
  }).toThrow(/only rtp and l16 slots record/);
  const kept = join(dir, "kept.pcap");
  await writeFile(kept, "someone's file");
  expect(() => {
    mixer.recordSlot(0, kept);
  }).toThrow(/cannot record to/);
  expect(await readFile(kept, "utf8")).toBe("someone's file");

  await destroyLive(engine, mixer);
  expect(() => {
    mixer.recordSlot(0, join(dir, "late.pcap"));
  }).toThrow(/destroyed/);
  await engine.close();
  await rm(dir, { recursive: true });
});
