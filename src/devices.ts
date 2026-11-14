import { nativeInputs, nativeOutputs } from "./native.ts";

/**
 * The outputs the OS offers, the default marked, with no engine open: where a host looks for
 * another output when `openEngine` rejects with `OutputUnavailableError`. Opens no stream and
 * changes no device setting.
 */
export async function outputDevices(): Promise<
  ReadonlyArray<{ id: string; name: string; isDefault: boolean }>
> {
  return nativeOutputs();
}

/** The inputs the OS offers, the default marked, with no engine open: what a `capture` slot may name. */
export async function inputDevices(): Promise<
  ReadonlyArray<{ id: string; name: string; isDefault: boolean }>
> {
  return nativeInputs();
}
