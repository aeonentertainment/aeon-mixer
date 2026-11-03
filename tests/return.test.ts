import { createSocket } from "node:dgram";
import { once } from "node:events";
import type { AddressInfo } from "node:net";
import { expect, test } from "vite-plus/test";
import { type MixerConfig, openEngine } from "../src/index.ts";

/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;
/** 5.3 s of output. The ring holds 50 ms, and the offline clock renders this in milliseconds. */
const LONG_PUMP = 2000;

type RtpReturn = NonNullable<MixerConfig["rtpReturn"]>;
const VALID: RtpReturn = { host: "127.0.0.1", port: 5004, payloadType: 111, ssrc: 0x1234_5678 };

test("rtpReturn is checked before anything crosses", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const refused: ReadonlyArray<[Partial<RtpReturn>, ErrorConstructor]> = [
    [{ host: "mixer.local" }, TypeError],
    [{ host: "" }, TypeError],
    [{ port: 0 }, RangeError],
    [{ port: 65536 }, RangeError],
    [{ port: 5004.5 }, TypeError],
    [{ payloadType: 95 }, RangeError],
    [{ payloadType: 128 }, RangeError],
    [{ ssrc: -1 }, RangeError],
    [{ ssrc: 2 ** 32 }, RangeError],
  ];
  await Promise.all(
    refused.map(([change, error]) =>
      expect(engine.createMixer({ slots: [], rtpReturn: { ...VALID, ...change } })).rejects.toThrow(
        error,
      ),
    ),
  );
  const edges = await engine.createMixer({
    slots: [],
    rtpReturn: { host: "127.0.0.1", port: 65535, payloadType: 127, ssrc: 0xffff_ffff },
  });
  await edges.destroy();
  await engine.close();
});

test("the return stream carries the mix from start to destroy and counts what the network could not take", async () => {
  // Somewhere for the datagrams to land; the test never reads them (docs/design.md section 9)
  const sink = createSocket("udp4");
  sink.bind(0, "127.0.0.1");
  await once(sink, "listening");
  const { port } = sink.address() satisfies AddressInfo;

  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({
    slots: [{ kind: "push" }],
    rtpReturn: { ...VALID, port },
  });
  // An armed graph is not rendered, so nothing reaches the ring however long the clock runs
  engine.pump(LONG_PUMP);
  expect(mixer.stats().rtpReturn).toEqual({ droppedBlocks: 0 });

  const started = mixer.start();
  engine.pump(FADE_BLOCKS);
  await started;
  // The offline clock outruns the network thread's 5 ms tick, so the full ring drops blocks
  engine.pump(LONG_PUMP);
  expect(mixer.stats().rtpReturn?.droppedBlocks).toBeGreaterThan(0);

  const destroyed = mixer.destroy();
  engine.pump(FADE_BLOCKS);
  await destroyed;
  expect(mixer.stats().rtpReturn).toBeNull();
  await engine.close();
  sink.close();
});
