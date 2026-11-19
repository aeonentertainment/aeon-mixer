import { expect, test } from "vite-plus/test";
import { inputDevices, outputDevices } from "../src/index.ts";
import { TRANSPORTS } from "../src/types.ts";

// The addon's test OS lists one device of each transport, as outputs and as inputs, so every OS CI
// runs shows the names a host switches on. Listing opens nothing.
test("each listed device says its transport", async () => {
  process.env.AEON_MIXER_TEST_OS = "hangs";
  try {
    const listed = TRANSPORTS.map((transport) => ({
      id: `test-${transport}`,
      name: `Test ${transport}`,
      isDefault: transport === "builtIn",
      transport,
    }));
    expect(await outputDevices()).toEqual(listed);
    expect(await inputDevices()).toEqual(listed);
  } finally {
    delete process.env.AEON_MIXER_TEST_OS;
  }
});
