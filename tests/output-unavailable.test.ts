import { expect, test } from "vite-plus/test";
import { OutputUnavailableError } from "../src/index.ts";

// Only a real output that will not start makes the addon throw this, so the test hands `from`
// the error as the addon throws it: a plain Error with its code and device on it.
test("an output that will not start is told apart by its code, with the device it names", () => {
  const thrown = Object.assign(new Error("P32p would not start: OSStatus 1937010544"), {
    code: "output-unavailable",
    device: { id: "uid-p32p", name: "P32p" },
  });
  const error = OutputUnavailableError.from(thrown);
  expect(error).toBeInstanceOf(OutputUnavailableError);
  expect(error).toMatchObject({
    code: "output-unavailable",
    device: { id: "uid-p32p", name: "P32p" },
    message: "P32p would not start: OSStatus 1937010544",
  });

  const none = OutputUnavailableError.from(
    Object.assign(new Error("no output device"), { code: "output-unavailable", device: null }),
  );
  expect(none).toMatchObject({ device: null });

  const other = new Error("follower thread: out of threads");
  expect(OutputUnavailableError.from(other)).toBe(other);
});
