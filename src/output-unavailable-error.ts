/**
 * `openEngine` rejects with this when the output it would play on does not start: the pinned
 * output or the default, which `device` names, or none at all when the OS lists no output. Another
 * output may still start; `outputDevices()` lists them with no engine open.
 */
export class OutputUnavailableError extends Error {
  readonly code = "output-unavailable";
  readonly device: { readonly id: string; readonly name: string } | null;

  constructor(message: string, device: { id: string; name: string } | null) {
    super(message);
    this.name = "OutputUnavailableError";
    this.device = device;
  }

  /** The addon's error for an output that did not start, as this class; anything else as it was. */
  static from(error: unknown): unknown {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "output-unavailable") {
      return error;
    }
    const device = "device" in error ? error.device : null;
    return new OutputUnavailableError(error.message, isDevice(device) ? device : null);
  }
}

function isDevice(value: unknown): value is { id: string; name: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "id" in value &&
    typeof value.id === "string" &&
    "name" in value &&
    typeof value.name === "string"
  );
}
