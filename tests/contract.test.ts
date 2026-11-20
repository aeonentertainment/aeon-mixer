import { networkInterfaces } from "node:os";
import { expect, test } from "vite-plus/test";
import { checkContract, type ContractEngine, type ContractLimit } from "../src/contract/index.ts";
import { Emitter } from "../src/emitter.ts";
import {
  type EngineEvents,
  type OpenEngineOptions,
  openEngine,
  outputDevices,
} from "../src/index.ts";

// The addon's test OS lists one output of each transport, so every OS CI runs has outputs to check
async function testOsOutputs() {
  process.env.AEON_MIXER_TEST_OS = "hangs";
  try {
    return await outputDevices();
  } finally {
    delete process.env.AEON_MIXER_TEST_OS;
  }
}

/** Whether this machine has an IPv4 address but loopback, which the listen promise sends from. */
const hasWiderAddress = Object.values(networkInterfaces())
  .flat()
  .some((info) => info?.family === "IPv4" && !info.internal);

test("the package keeps every promise of its contract, and an offline engine has no move to check", async () => {
  const report = await checkContract({ openEngine, outputDevices: testOsOutputs });
  expect(report.broken).toEqual([]);
  expect(report.unchecked).toEqual([
    { promise: expect.stringMatching(/^After a move, `outputChanged`/), why: expect.any(String) },
    ...(hasWiderAddress
      ? []
      : [{ promise: expect.stringMatching(/^An `rtp` slot takes/), why: expect.any(String) }]),
  ]);
}, 30_000);

/**
 * The package with the slips a host's hand-made stand-in tends to have: a slot that ignores the
 * profile's gain, a `resetSlot` that only notes the call, and a move that reports holding a
 * backing nobody is playing.
 */
async function openStandIn(options: OpenEngineOptions & { output: { offline: true } }) {
  const engine = await openEngine(options);
  const moves = new Emitter<EngineEvents>();
  const standIn = {
    get output() {
      return engine.output;
    },
    pump: (blocks: number) => engine.pump(blocks),
    close: () => engine.close(),
    on<K extends keyof EngineEvents>(event: K, listener: EngineEvents[K]) {
      return event === "outputChanged" ? moves.on(event, listener) : engine.on(event, listener);
    },
    async createMixer(config: Parameters<ContractEngine["createMixer"]>[0]) {
      const mixer = await engine.createMixer(config);
      config.slots.forEach((_, index) => mixer.setSlot(index, { gain: 1 }));
      return {
        slots: mixer.slots,
        setSlot: mixer.setSlot.bind(mixer),
        setSlotEffects: mixer.setSlotEffects.bind(mixer),
        resetSlot: () => {},
        push: mixer.push.bind(mixer),
        start: mixer.start.bind(mixer),
        destroy: mixer.destroy.bind(mixer),
        stats: mixer.stats.bind(mixer),
        on: mixer.on.bind(mixer),
      };
    },
    move() {
      moves.emit("outputChanged", engine.output, 40, 40);
    },
  };
  return standIn;
}

test("a stand-in is told each promise it breaks, in a sentence", async () => {
  const report = await checkContract(
    { openEngine: openStandIn, outputDevices: testOsOutputs },
    { moveOutput: (engine) => engine.move() },
  );
  expect(report.unchecked).toEqual([]);
  expect(report.broken).toEqual([
    {
      promise: expect.stringMatching(/^After a move/),
      found: "its heldMs was 40 with no song playing",
    },
    {
      promise: "A slot nobody has set plays at the profile's `slot.gain`.",
      found: "a slot nobody set played at gain 1.000 under a profile of 0.7",
    },
    {
      promise: "After `resetSlot`, a slot takes a new sender at once.",
      found: "after `resetSlot`, the slot still refused a new sender",
    },
    {
      promise: "After `resetSlot`, a slot refuses the sender it released.",
      found:
        "after `resetSlot`, the slot counted 1 of the 4 packets sent and 0 new streams, where only the new sender's 3, on 2 streams, should count: it took back the sender it released",
    },
  ]);
}, 30_000);

/** The package, except that `close` rejects once it has closed, so a destroy in flight rejects after it. */
async function openRefusingClose(options: OpenEngineOptions & { output: { offline: true } }) {
  const engine = await openEngine(options);
  return {
    get output() {
      return engine.output;
    },
    pump: (blocks: number) => engine.pump(blocks),
    on: engine.on.bind(engine),
    createMixer: (config: Parameters<ContractEngine["createMixer"]>[0]) =>
      engine.createMixer(config),
    async close() {
      await engine.close();
      throw new Error("the stand-in could not close");
    },
  };
}

test("a stand-in whose close rejects gets its sentence, and the destroy it cut short is heard", async () => {
  const unheard: unknown[] = [];
  const hear = (reason: unknown) => unheard.push(reason);
  process.on("unhandledRejection", hear);
  try {
    const report = await checkContract({
      openEngine: openRefusingClose,
      outputDevices: testOsOutputs,
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(report.broken).toEqual([
      {
        promise: "`destroy` settles when the engine closes in the middle of its swap.",
        found: "close rejected: the stand-in could not close",
      },
    ]);
    expect(unheard).toEqual([]);
  } finally {
    process.off("unhandledRejection", hear);
  }
}, 30_000);

/**
 * A stand-in that only records, as a host's adapter specs drive one: the package answers its calls
 * and keeps its clock, but nothing it plays comes back, its tone stage never reports, and its
 * slots open no socket a sender could reach.
 */
async function openRecorder(options: OpenEngineOptions & { output: { offline: true } }) {
  const engine = await openEngine(options);
  const moves = new Emitter<EngineEvents>();
  return {
    get output() {
      return engine.output;
    },
    pump(blocks: number) {
      engine.pump(blocks);
      return new Float32Array(2 * 128 * blocks);
    },
    close: () => engine.close(),
    on<K extends keyof EngineEvents>(event: K, listener: EngineEvents[K]) {
      return event === "outputChanged" ? moves.on(event, listener) : engine.on(event, listener);
    },
    async createMixer(config: Parameters<ContractEngine["createMixer"]>[0]) {
      const mixer = await engine.createMixer(config);
      return {
        slots: mixer.slots.map((slot) => ({ ...slot, port: null })),
        setSlot: mixer.setSlot.bind(mixer),
        setSlotEffects: mixer.setSlotEffects.bind(mixer),
        resetSlot: mixer.resetSlot.bind(mixer),
        push: mixer.push.bind(mixer),
        start: mixer.start.bind(mixer),
        destroy: mixer.destroy.bind(mixer),
        stats() {
          const stats = mixer.stats();
          // A fresh object every call, so its slots are this stand-in's to change
          for (const slot of stats.slots) Object.assign(slot, { tone: null });
          return stats;
        },
        on: mixer.on.bind(mixer),
      };
    },
    move() {
      moves.emit("outputChanged", engine.output, 40, 0);
    },
  };
}

const RENDERED = [
  "A stem handed in as bytes plays the samples the same file plays by path.",
  "A slot nobody has set plays at the profile's `slot.gain`.",
  "A gain set while the mixer is armed is the gain its slot starts at.",
  "A slot's `tone` is null while its tone stage is off, and reports once it is on.",
];
const SOCKETED = [
  "An `rtp` slot takes datagrams from this machine only, unless its `listen` is `every-interface`.",
  "A slot playing one sender refuses every other address.",
  "After `resetSlot`, a slot takes a new sender at once.",
  "After `resetSlot`, a slot refuses the sender it released.",
];

async function checkRecorder(limits: ReadonlyArray<ContractLimit> = []) {
  return checkContract(
    { openEngine: openRecorder, outputDevices: testOsOutputs },
    { moveOutput: (engine) => engine.move(), limits },
  );
}

test("a stand-in that renders no audio and opens no socket says so, and only what needs those goes unchecked", async () => {
  const report = await checkRecorder(["rendersNoAudio", "opensNoSocket"]);
  expect(report.broken).toEqual([]);
  expect(report.unchecked).toEqual([
    {
      promise: RENDERED[0],
      why: "the subject renders no audio, and this promise is about what it plays",
    },
    ...RENDERED.slice(1).map((promise) => ({
      promise,
      why: expect.stringMatching(/renders no audio/),
    })),
    ...SOCKETED.map((promise) => ({ promise, why: expect.stringMatching(/opens no socket/) })),
  ]);
}, 30_000);

test("a stand-in that claims to render and to listen, and does neither, breaks those promises", async () => {
  const report = await checkRecorder();
  expect(report.unchecked).toEqual([]);
  expect(report.broken.map(({ promise }) => promise)).toEqual([...RENDERED, ...SOCKETED]);
}, 30_000);

test("stating a limit excuses only the promises that need it", async () => {
  const report = await checkRecorder(["opensNoSocket"]);
  expect(report.unchecked.map(({ promise }) => promise)).toEqual(SOCKETED);
  expect(report.broken.map(({ promise }) => promise)).toEqual(RENDERED);
}, 30_000);
