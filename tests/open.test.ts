import { expect, test } from "vite-plus/test";
import { OutputUnavailableError, openEngine, outputDevices } from "../src/index.ts";

/** The longest a 10 ms timer may wait while an open's start hangs. A blocked loop waits 3 s. */
const TIMER_TOLERANCE_MS = 200;
const REFUSE_AFTER_MS = 3000;

// The addon's test OS lists one output, the default, whose start never returns. No real device is
// opened: the follower refuses the start at its bound and the host hears OutputUnavailableError.
test("an open whose start hangs leaves the event loop running, then is refused after 3 s", async () => {
  process.env.AEON_MIXER_TEST_OS = "hangs";
  try {
    // An addon without the test OS lists the real outputs here, and must not go on to open one
    const hanging = { id: "test-hangs", name: "Hanging output" };
    expect(await outputDevices()).toEqual([{ ...hanging, isDefault: true }]);

    let last = performance.now();
    let longestWait = 0;
    const timer = setInterval(() => {
      const now = performance.now();
      longestWait = Math.max(longestWait, now - last);
      last = now;
    }, 10);
    const started = performance.now();
    const refused = openEngine({ output: {} }).then(
      () => null,
      (error: unknown) => error,
    );

    // Another open goes ahead while the first waits on its start
    const offline = await openEngine({ output: { offline: true } });
    expect(performance.now() - started).toBeLessThan(TIMER_TOLERANCE_MS);
    await offline.close();

    const error = await refused;
    const took = performance.now() - started;
    clearInterval(timer);

    expect(error).toBeInstanceOf(OutputUnavailableError);
    expect(error).toMatchObject({ code: "output-unavailable", device: hanging });
    expect(took).toBeGreaterThanOrEqual(REFUSE_AFTER_MS);
    expect(took).toBeLessThan(REFUSE_AFTER_MS + 500);
    expect(longestWait).toBeLessThan(TIMER_TOLERANCE_MS);
  } finally {
    delete process.env.AEON_MIXER_TEST_OS;
  }
}, 10_000);
