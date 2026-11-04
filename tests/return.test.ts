import { expect, test } from "vite-plus/test";
import { type MixerConfig, openEngine } from "../src/index.ts";

/** 5.3 s of output, a hundred times what the return ring holds. */
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

test("a mixer with a return stream builds it, counts no drops while armed, and drops it on destroy", async () => {
  const engine = await openEngine({ output: { offline: true } });
  const mixer = await engine.createMixer({ slots: [{ kind: "push" }], rtpReturn: VALID });
  // An armed graph is not rendered, so nothing reaches the ring however long the clock runs and
  // nothing is sent (docs/design.md section 9); the drops of a live ring are proven in Rust
  engine.pump(LONG_PUMP);
  expect(mixer.stats().rtpReturn).toEqual({ droppedBlocks: 0 });
  await mixer.destroy();
  expect(mixer.stats().rtpReturn).toBeNull();
  await engine.close();
});
