import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test, vi } from "vite-plus/test";
import { ACK_TIMEOUT_MS } from "../src/acks.ts";
import { type Mixer, type MixerConfig, type OfflineEngine, openEngine } from "../src/index.ts";
import type { NativeEvent } from "../src/native.ts";

// The offline clock cannot fault, so the fault test hands the engine the batch the events thread
// would: the callback it gives the addon is the seam, and the public API stays as it is.
const addon = vi.hoisted(() => ({ deliver: (_batch: NativeEvent[]): void => {} }));

vi.mock(import("../src/native.ts"), async (original) => {
  const native = await original();
  class TappedEngine extends native.NativeEngine {
    static override open(...[options, deliver]: Parameters<typeof native.NativeEngine.open>) {
      addon.deliver = deliver;
      return native.NativeEngine.open(options, deliver);
    }
  }
  return { ...native, NativeEngine: TappedEngine };
});

const RATE = 48000;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;

const dir = mkdtempSync(join(tmpdir(), "aeon-mixer-error-"));
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

const notAudio = join(dir, "not-audio.wav");
writeFileSync(notAudio, "this is not audio");

/** A second of silence as a mono 16-bit WAV at 48 kHz. */
function silentStem() {
  const data = Buffer.alloc(RATE * 2);
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
  const path = join(dir, "silent.wav");
  writeFileSync(path, Buffer.concat([header, data]));
  return path;
}

function backingOf(path: string, key = 0): NonNullable<MixerConfig["backing"]> {
  return {
    stems: [{ kind: "instrumental", path }],
    key,
    speed: 1,
    stemGains: { instrumental: 1, guideVocals: 1 },
  };
}

function onceError(mixer: Mixer) {
  return new Promise<Error>((resolve) => {
    mixer.on("error", resolve);
  });
}

function errorsOf(mixer: Mixer) {
  const errors: Error[] = [];
  mixer.on("error", (error) => errors.push(error));
  return errors;
}

function rendered(mixer: Mixer) {
  return new Promise<void>((resolve) => {
    mixer.on("renderProgress", (fraction) => {
      if (fraction === 1) resolve();
    });
  });
}

async function startLive(engine: OfflineEngine, mixer: Mixer) {
  const started = mixer.start();
  engine.pump(FADE_BLOCKS);
  return started;
}

/** Runs `body` with the ack timeout under the test's hand. */
async function withFakeTimers(body: () => Promise<void>) {
  vi.useFakeTimers();
  try {
    await body();
  } finally {
    vi.useRealTimers();
  }
}

test.each([
  ["does not exist", join(dir, "missing.wav"), /missing\.wav/],
  ["will not decode", notAudio, /not-audio\.wav/],
])(
  "a backing that %s emits error before start, and start rejects with the same message",
  async (_, path, named) => {
    const engine = await openEngine({ output: { offline: true } });
    const mixer = await engine.createMixer({ slots: [], backing: backingOf(path) });
    const errors = errorsOf(mixer);
    const error = await onceError(mixer);
    expect(error.message).toMatch(named);

    await expect(mixer.start()).rejects.toMatchObject({ message: error.message });
    await expect(mixer.start()).rejects.toMatchObject({ message: error.message });
    expect(errors).toHaveLength(1);
    await mixer.destroy();
    await engine.close();
  },
);

test("a start already waiting on the render rejects with the message error carried", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backingOf(notAudio) });
  const error = onceError(mixer);
  const refusal = await mixer.start().catch((reason: unknown) => reason);
  expect(refusal).toMatchObject({ message: (await error).message });
  await mixer.destroy();
  await engine.close();
});

test("a host may destroy from its error listener while a start waits on the failed render", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backingOf(notAudio) });
  const destroyed = new Promise<void>((resolve, reject) => {
    mixer.on("error", () => {
      mixer.destroy().then(resolve, reject);
    });
  });
  const error = onceError(mixer);

  const refusal = await mixer.start().catch((reason: unknown) => reason);
  expect(refusal).toBe(await error);
  await destroyed;
  await expect(mixer.start()).rejects.toThrow(/destroyed/);
  await engine.close();
});

test("a start that goes unanswered emits error once, rejects with it, and leaves the engine open", async () => {
  await withFakeTimers(async () => {
    const engine = await openEngine({ output: { offline: true } });
    const faults = vi.fn();
    engine.on("fault", faults);
    const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
    const errors = errorsOf(mixer);

    const started = mixer.start();
    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    expect(errors).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(errors).toHaveLength(1);
    await expect(started).rejects.toBe(errors[0]);
    expect(errors[0]?.message).toMatch(/not acknowledged within 1000 ms; the engine is running/);

    expect(faults).not.toHaveBeenCalled();
    const destroyed = mixer.destroy();
    engine.pump(FADE_BLOCKS);
    await destroyed;
    await engine.close();
  });
});

test("a backing command that goes unanswered emits error, and a second one adds nothing", async () => {
  await withFakeTimers(async () => {
    const engine = await openEngine({ output: { offline: true } });
    const mixer = await engine.createMixer({ slots: [], backing: backingOf(silentStem()) });
    const errors = errorsOf(mixer);
    await rendered(mixer);
    await startLive(engine, mixer);

    const seeking = mixer.seek(100);
    vi.advanceTimersByTime(ACK_TIMEOUT_MS);
    await expect(seeking).rejects.toBe(errors[0]);

    const pausing = mixer.pause();
    vi.advanceTimersByTime(ACK_TIMEOUT_MS);
    await expect(pausing).rejects.toThrow(/not acknowledged/);
    expect(errors).toHaveLength(1);

    const destroyed = mixer.destroy();
    engine.pump(FADE_BLOCKS);
    await destroyed;
    await engine.close();
  });
});

test("a mixer being destroyed stays quiet when its commands and its destroy go unanswered", async () => {
  await withFakeTimers(async () => {
    const engine = await openEngine({ output: { offline: true } });
    const mixer = await engine.createMixer({ slots: [], backing: backingOf(silentStem()) });
    const errors = errorsOf(mixer);
    await rendered(mixer);
    await startLive(engine, mixer);

    const seeking = mixer.seek(100);
    const destroyed = mixer.destroy();
    vi.advanceTimersByTime(ACK_TIMEOUT_MS);
    await expect(seeking).rejects.toThrow(/not acknowledged/);
    await expect(destroyed).rejects.toThrow(/not acknowledged/);
    expect(errors).toEqual([]);

    engine.pump(FADE_BLOCKS);
    await engine.close();
  });
});

test("a render that fails after its mixer is destroyed stays quiet", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const destroyed = await engine.createMixer({ slots: [], backing: backingOf(notAudio) });
  const errors = errorsOf(destroyed);
  await destroyed.destroy();

  // Opening a bad file is over long before a keyed second has been through the stretcher
  const later = await engine.createMixer({ slots: [], backing: backingOf(silentStem(), 2) });
  await rendered(later);
  expect(errors).toEqual([]);
  await later.destroy();
  await engine.close();
});

test("an engine fault reaches every mixer it holds once, and a destroyed one not at all", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const faults: Error[] = [];
  engine.on("fault", (error) => faults.push(error));

  const live = await engine.createMixer({ slots: [{ kind: "push" }] });
  const armed = await engine.createMixer({ slots: [{ kind: "push" }] });
  const gone = await engine.createMixer({ slots: [{ kind: "push" }] });
  const [liveErrors, armedErrors, goneErrors] = [live, armed, gone].map(errorsOf);
  await gone.destroy();
  await startLive(engine, live);

  addon.deliver([{ kind: "fault" }, { kind: "fault" }]);

  expect(faults).toHaveLength(1);
  expect(faults[0]?.message).toMatch(/faulted/);
  expect(liveErrors).toEqual(faults);
  expect(armedErrors).toEqual(faults);
  expect(goneErrors).toEqual([]);

  // The host ends the song and destroys it; a faulted engine has nothing left to wait for
  await live.destroy();
  await armed.destroy();
  expect(liveErrors).toHaveLength(1);
  await engine.close();
});

test("a fault rejects the command in flight with the error the mixer emitted", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }] });
  const errors = errorsOf(mixer);
  const started = mixer.start();

  addon.deliver([{ kind: "fault" }]);

  await expect(started).rejects.toBe(errors[0]);
  expect(errors).toHaveLength(1);
  await engine.close();
});

test("a host may destroy from its error listener while a fault rejects the start waiting on a render", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backingOf(silentStem(), 2) });
  const destroyed = new Promise<void>((resolve, reject) => {
    mixer.on("error", () => {
      mixer.destroy().then(resolve, reject);
    });
  });
  const started = mixer.start();

  addon.deliver([{ kind: "fault" }]);

  await expect(started).rejects.toThrow(/faulted/);
  await destroyed;
  await expect(mixer.start()).rejects.toThrow(/destroyed/);
  await engine.close();
});

test("closing the engine is not an error: the render it stops and the commands it rejects stay quiet", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [], backing: backingOf(silentStem(), 2) });
  const errors = errorsOf(mixer);
  const started = mixer.start();
  await engine.close();
  await expect(started).rejects.toThrow(/closed/);
  expect(errors).toEqual([]);
});
