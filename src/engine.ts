import { OfflineEngine, type OutputInfo } from "./native.ts";

export type Capability =
  | "guideVocal"
  | "key"
  | "speed"
  | "seek"
  | "shortVersion"
  | "pause"
  | "level";

export interface OpenEngineOptions {
  output: { device?: string; periodFrames?: 64 | 128 } | { offline: true };
}

export interface Engine {
  /** What this build can do. Empty until the workstreams that add capabilities land. */
  readonly capabilities: ReadonlySet<Capability>;
  readonly output: Readonly<OutputInfo>;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  pump(blocks: number): Float32Array;
  close(): Promise<void>;
}

/**
 * Opens the engine for the life of the process. Only the offline driver exists yet; a device
 * output is refused until the devices workstream lands (docs/design.md 8.3).
 */
export async function openEngine(options: OpenEngineOptions): Promise<Engine> {
  if (!("offline" in options.output)) {
    throw new Error("device output is not built yet; open with { output: { offline: true } }");
  }
  const native = new OfflineEngine();
  return {
    capabilities: new Set<Capability>(),
    output: native.output,
    pump(blocks) {
      if (!Number.isInteger(blocks) || blocks < 1) {
        throw new RangeError(`blocks must be a positive integer, got ${blocks}`);
      }
      return native.pump(blocks);
    },
    async close() {
      native.close();
    },
  };
}
