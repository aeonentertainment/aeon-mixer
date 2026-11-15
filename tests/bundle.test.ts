import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, expect, test, vi } from "vite-plus/test";
import packageJson from "../package.json" with { type: "json" };
import { bundleOutput } from "../src/bundle/bundle-manifest.ts";
import { type Mixer, type OfflineEngine, openEngine, parseProfile } from "../src/index.ts";

const RATE = 48000;
const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
/** Seek fades over 20 ms, 7.5 blocks: the eighth block lands it. */
const SEEK_FADE_BLOCKS = 8;

const dir = mkdtempSync(join(tmpdir(), "aeon-bundle-"));
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** Writes `seconds` of a constant as a mono 16-bit WAV at 48 kHz and returns its path. */
function stem(name: string, seconds: number) {
  const data = Buffer.alloc(RATE * seconds * 2);
  for (let i = 0; i < RATE * seconds; i += 1) data.writeInt16LE(8192, 2 * i);
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(RATE, 24);
  header.writeUInt32LE(RATE * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  const path = join(dir, name);
  writeFileSync(path, Buffer.concat([header, data]));
  return path;
}

/** An offline engine that counts the frames it has been pumped: what a bundle's `frame` reads. */
async function countingEngine(options: { playoutDelayMs?: number } = {}) {
  const engine = await openEngine({ output: { offline: true }, ...options });
  let frames = 0;
  return {
    engine,
    pump: (blocks: number) => {
      frames += blocks * BLOCK;
      return engine.pump(blocks);
    },
    frames: () => frames,
  };
}

/** Resolves once the backing's render is done and its stems are in the graph. */
function rendered(mixer: Mixer) {
  return new Promise<void>((resolve) => {
    mixer.on("renderProgress", (fraction) => {
      if (fraction === 1) resolve();
    });
  });
}

async function startLive(pump: (blocks: number) => unknown, mixer: Mixer) {
  const started = mixer.start();
  pump(FADE_BLOCKS);
  return started;
}

async function destroyLive(engine: OfflineEngine, mixer: Mixer) {
  const destroyed = mixer.destroy();
  engine.pump(FADE_BLOCKS);
  await destroyed;
}

interface Line {
  type?: string;
  frame: number;
  us: number;
  [field: string]: unknown;
}

function jsonLines(path: string): Line[] {
  const text = readFileSync(path, "utf8");
  expect(text === "" || text.endsWith("\n"), "every line is whole").toBe(true);
  return text
    .split("\n")
    .slice(0, -1)
    .map((line): Line => JSON.parse(line));
}

/** Checks a line's `us` and returns the rest of it, which a test can know exactly. */
function onTheWallClock({ us, ...line }: Line) {
  expectNow(us);
  return line;
}

function sha256(file: string) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}

function usOf(value: unknown) {
  if (typeof value !== "object" || value === null || !("us" in value)) throw new Error("no us");
  if (typeof value.us !== "number") throw new Error("us is not a number");
  return value.us;
}

/** Within a second of now: the wall clock, in microseconds. */
function expectNow(us: number) {
  expect(Math.abs(us / 1000 - Date.now())).toBeLessThan(1000);
}

test("a bundle holds the manifest, the profile, a pcap for each rtp and l16 slot, and an empty host directory", async () => {
  const { engine, pump, frames } = await countingEngine({ playoutDelayMs: 20 });
  const instrumental = stem("whole-instrumental.wav", 1);
  const guide = stem("whole-guide.wav", 2);
  pump(5);
  const path = join(dir, "whole");
  const mixer = await engine.createMixer({
    slots: [{ kind: "rtp" }, { kind: "push" }, { kind: "l16" }],
    backing: {
      stems: [
        { kind: "instrumental", path: instrumental },
        { kind: "guideVocals", path: guide },
      ],
      key: -2,
      speed: 1,
      stemGains: { guideVocals: 0.3 },
      stopAtMs: 1500,
    },
    bundle: { dir: path },
  });

  expect(readdirSync(path).toSorted()).toEqual([
    "events.jsonl",
    "host",
    "manifest.json",
    "profile.json",
    "slots",
    "stats.jsonl",
  ]);
  expect(readdirSync(join(path, "host"))).toEqual([]);

  const manifestText = readFileSync(join(path, "manifest.json"), "utf8");
  const manifest: unknown = JSON.parse(manifestText);
  expect(manifest).toStrictEqual({
    format: 1,
    package: packageJson.version,
    platform: `${process.platform}-${process.arch}`,
    output: { sampleRate: 48000, periodFrames: 128, latencyMs: 0, name: null },
    frame: frames(),
    us: usOf(manifest),
    slots: [
      { index: 0, kind: "rtp", recorded: true },
      { index: 1, kind: "push", recorded: false },
      { index: 2, kind: "l16", recorded: true },
    ],
    backing: {
      stems: [
        { kind: "instrumental", sha256: sha256(instrumental) },
        { kind: "guideVocals", sha256: sha256(guide) },
      ],
      key: -2,
      speed: 1,
      // The instrumental's gain was left to the profile, so it is left out here
      stemGains: { guideVocals: 0.3 },
      stopAtMs: 1500,
    },
  });
  expectNow(usOf(manifest));
  // A stem is named by its hash: no path on the host's disk
  expect(manifestText).not.toContain(dir);
  expect(manifestText).not.toContain("whole-instrumental");

  // The engine's profile as the mixer was created, the pinned delay folded in
  const profile = parseProfile(readJson(join(path, "profile.json")));
  expect(profile).toEqual(engine.profile);
  expect(profile.ingest.playoutDelayMs).toBe(20);

  // Classic pcap from creation, before a packet: the microsecond magic, little endian
  expect(readdirSync(join(path, "slots")).toSorted()).toEqual(["0.pcap", "2.pcap"]);
  for (const slot of ["0.pcap", "2.pcap"]) {
    const header = readFileSync(join(path, "slots", slot));
    expect([...header.subarray(0, 4)]).toEqual([0xd4, 0xc3, 0xb2, 0xa1]);
  }

  await mixer.destroy();
  await engine.close();
});

test("an effect change and a seek mid-song each leave a line with the call as made, a frame and a wall-clock time", async () => {
  const { engine, pump, frames } = await countingEngine();
  const path = join(dir, "song");
  const mixer = await engine.createMixer({
    slots: [{ kind: "push" }],
    backing: {
      stems: [{ kind: "instrumental", path: stem("song.wav", 3) }],
      key: 0,
      speed: 1,
    },
    bundle: { dir: path },
  });
  await rendered(mixer);
  pump(2);
  const startedAt = frames();
  await startLive(pump, mixer);

  pump(40);
  const effectsAt = frames();
  // The reverb's parameters are left to the profile; the correction's strength is a choice
  mixer.setSlotEffects(0, [
    { kind: "reverb", enabled: true },
    { kind: "pitch-correction", enabled: true, strength: 1 },
  ]);
  mixer.setSlot(0, { mute: true });
  mixer.setSlotGuide(0, { notes: [{ midi: 60, startMs: 0, endMs: 500 }], keyMask: 0xab5 }, 40);
  mixer.setStemGain("instrumental", 0.5);

  pump(60);
  const seekAt = frames();
  const seeking = mixer.seek(2000);
  pump(SEEK_FADE_BLOCKS);
  expect(await seeking).toEqual({ positionMs: 2000 });

  pump(10);
  const endAt = frames();
  await destroyLive(engine, mixer);
  await engine.close();

  const events = jsonLines(join(path, "events.jsonl"));
  expect(events.map(onTheWallClock)).toStrictEqual([
    { type: "backingRendered", frame: 0, frames: 3 * RATE },
    { type: "start", frame: startedAt, positionMs: 0 },
    {
      type: "setSlotEffects",
      frame: effectsAt,
      index: 0,
      effects: [
        { kind: "reverb", enabled: true },
        { kind: "pitch-correction", enabled: true, strength: 1 },
      ],
    },
    { type: "setSlot", frame: effectsAt, index: 0, settings: { mute: true } },
    {
      type: "setSlotGuide",
      frame: effectsAt,
      index: 0,
      guide: { notes: [{ midi: 60, startMs: 0, endMs: 500 }], keyMask: 0xab5 },
      guideOffsetMs: 40,
    },
    { type: "setStemGain", frame: effectsAt, kind: "instrumental", gain: 0.5 },
    // Stamped as the seek was sent, written when its ack came with where it landed
    { type: "seek", frame: seekAt, toMs: 2000, positionMs: 2000 },
    { type: "end", frame: endAt },
  ]);
});

test("stats() and the backing's position are written once a second, on the bundle's clock", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  try {
    const { engine, pump, frames } = await countingEngine();
    const path = join(dir, "stats");
    const mixer = await engine.createMixer({
      slots: [{ kind: "rtp" }, { kind: "push" }],
      bundle: { dir: path },
    });
    vi.advanceTimersByTime(1000);
    await startLive(pump, mixer);
    pump(372);
    vi.advanceTimersByTime(1000);
    await destroyLive(engine, mixer);
    // A destroyed mixer's bundle is closed: no line after `end`
    vi.advanceTimersByTime(5000);
    await engine.close();

    const [armed, live, ...rest] = jsonLines(join(path, "stats.jsonl"));
    expect(rest).toEqual([]);
    expect(armed).toMatchObject({ frame: 0, positionMs: null });
    expect(live).toMatchObject({ frame: frames(), positionMs: 0, underruns: 0, rtpReturn: null });
    expectNow(live?.us ?? 0);
    expect(live).toHaveProperty("slots.length", 2);
    expect(live).toHaveProperty("slots.0.playoutDelayMs", 15);
    expect(live).toHaveProperty("slots.1.pushed", 0);
  } finally {
    vi.useRealTimers();
  }
});

test("a mixer destroyed armed, destroyed mid-song or closed with its engine leaves a bundle that ends in end", async () => {
  const { engine, pump } = await countingEngine();
  const types = (name: string) =>
    jsonLines(join(dir, name, "events.jsonl")).map(({ type }) => type);

  const armed = await engine.createMixer({ slots: [], bundle: { dir: join(dir, "armed") } });
  await armed.destroy();
  await armed.destroy();
  expect(types("armed")).toEqual(["end"]);

  const live = await engine.createMixer({
    slots: [{ kind: "push" }],
    bundle: { dir: join(dir, "live") },
  });
  await startLive(pump, live);
  live.resetSlot(0);
  await destroyLive(engine, live);
  expect(types("live")).toEqual(["start", "resetSlot", "end"]);

  await engine.createMixer({ slots: [], bundle: { dir: join(dir, "closed") } });
  await engine.close();
  expect(types("closed")).toEqual(["end"]);
});

test("a process that dies mid-song leaves files that read to their last line", async () => {
  const path = join(dir, "died");
  const fixture = fileURLToPath(new URL("./fixtures/bundle-dies.ts", import.meta.url));
  const child = spawn(process.execPath, [fixture, path], { stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  await once(child, "exit");
  expect(existsSync(join(path, "events.jsonl")), `the song never ran: ${stderr}`).toBe(true);

  // Every line it wrote is whole, and no `end` says it was cut short
  const events = jsonLines(join(path, "events.jsonl"));
  expect(events.map(({ type, frame }) => ({ type, frame }))).toEqual([
    { type: "start", frame: 0 },
    { type: "setSlotEffects", frame: 3 * BLOCK },
    { type: "setSlot", frame: 11 * BLOCK },
  ]);
  expect(jsonLines(join(path, "stats.jsonl"))).toEqual([]);
  expect(readJson(join(path, "manifest.json"))).toMatchObject({ format: 1, frame: 0 });
  expect(readFileSync(join(path, "slots", "0.pcap"))).toHaveLength(24);
});

test("createMixer rejects when the bundle's directory exists, and leaves it as it was", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const path = join(dir, "taken");
  mkdirSync(path);
  writeFileSync(join(path, "someone.txt"), "someone's file");
  await expect(
    engine.createMixer({ slots: [{ kind: "rtp" }], bundle: { dir: path } }),
  ).rejects.toThrow(/already exists/);
  expect(readdirSync(path)).toEqual(["someone.txt"]);
  await engine.close();
});

test("recordSlot throws on a mixer with a bundle, whose slots already record", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [{ kind: "rtp" }],
    bundle: { dir: join(dir, "one-recording") },
  });
  expect(() => {
    mixer.recordSlot(0, join(dir, "second.pcap"));
  }).toThrow(/recording into the mixer's bundle/);
  expect(() => {
    mixer.recordSlot(0, null);
  }).toThrow(/recording into the mixer's bundle/);
  expect(existsSync(join(dir, "second.pcap"))).toBe(false);
  await mixer.destroy();
  await engine.close();
});

test("a stem that will not decode leaves an error line that names it by kind, never by its path", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const noise = join(dir, "noise.wav");
  writeFileSync(noise, "not audio at all");
  const path = join(dir, "undecodable");
  const mixer = await engine.createMixer({
    slots: [],
    backing: { stems: [{ kind: "instrumental", path: noise }], key: 0, speed: 1 },
    bundle: { dir: path },
  });
  const error = await new Promise<Error>((resolve) => mixer.on("error", resolve));
  expect(error.message).toContain(noise);
  await mixer.destroy();
  await engine.close();

  const [failed, end] = jsonLines(join(path, "events.jsonl"));
  expect(failed?.type).toBe("error");
  expect(failed?.["message"]).toMatch(/^instrumental: /);
  expect(end?.type).toBe("end");
  expect(readFileSync(join(path, "events.jsonl"), "utf8")).not.toContain(dir);
});

test("a bundle names an output and never writes a device id", () => {
  const output = bundleOutput({
    sampleRate: 48000,
    periodFrames: 64,
    latencyMs: 9.3,
    device: { id: "usb-0d8c-0014-00", name: "Room speakers" },
    refused: { id: "hdmi-1", name: "Display", error: "the output would not start" },
  });
  expect(output).toStrictEqual({
    sampleRate: 48000,
    periodFrames: 64,
    latencyMs: 9.3,
    name: "Room speakers",
  });
});
