import { expect, test } from "vite-plus/test";
import { inputDevices, outputDevices, type Transport } from "../src/index.ts";

// Typed as a record so a transport added to the union fails here until the test OS lists it
const transports = Object.keys({
  builtIn: true,
  usb: true,
  bluetooth: true,
  display: true,
  network: true,
  aggregate: true,
  virtual: true,
  other: true,
  unknown: true,
} satisfies Record<Transport, true>);

// The addon's test OS lists one device of each transport, as outputs and as inputs, so every OS CI
// runs shows the names a host switches on. Listing opens nothing.
test("each listed device says its transport", async () => {
  process.env.AEON_MIXER_TEST_OS = "hangs";
  try {
    const listed = transports.map((transport) => ({
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
