import { nativeInputs, nativeOutputs } from "./native.ts";
import type { DeviceInfo } from "./types.ts";

/**
 * The outputs the OS offers, the default marked and each with its transport, with no engine open:
 * where a host looks for another output when `openEngine` rejects with `OutputUnavailableError`.
 * Opens no stream and changes no device setting.
 */
export async function outputDevices(): Promise<ReadonlyArray<DeviceInfo>> {
  return nativeOutputs();
}

/** The inputs the OS offers, listed as `outputDevices()` lists outputs: what a `capture` slot may name. */
export async function inputDevices(): Promise<ReadonlyArray<DeviceInfo>> {
  return nativeInputs();
}
