import { expect, test } from "vite-plus/test";
import { PACKAGE_NAME } from "../src/index.ts";

test("exposes the package name", () => {
  expect(PACKAGE_NAME).toBe("@aeon/mixer");
});
