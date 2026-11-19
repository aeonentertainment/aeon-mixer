import { expect, test } from "vite-plus/test";
import { checkContract, type ContractEngine } from "../src/contract/index.ts";
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

test("the package keeps every promise of its contract, and an offline engine has no move to check", async () => {
  const report = await checkContract({ openEngine, outputDevices: testOsOutputs });
  expect(report.broken).toEqual([]);
  expect(report.unchecked).toEqual([
    { promise: expect.stringMatching(/^After a move, `outputChanged`/), why: expect.any(String) },
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
