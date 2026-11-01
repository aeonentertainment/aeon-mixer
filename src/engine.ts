import {
  OfflineEngine as NativeOfflineEngine,
  type OutputInfo as NativeOutputInfo,
} from "./native.ts";

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

export interface OutputInfo {
  readonly sampleRate: 48000;
  readonly periodFrames: number;
  readonly latencyMs: number;
}

export interface Engine {
  /** What this build can do. Empty until the workstreams that add capabilities land. */
  readonly capabilities: ReadonlySet<Capability>;
  readonly output: OutputInfo;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  pump?(blocks: number): Float32Array;
  close(): Promise<void>;
}

/** An engine on the test clock. Its caller pumps it; nothing plays. */
export interface OfflineEngine extends Engine {
  pump(blocks: number): Float32Array;
}

/**
 * Opens the engine for the life of the process. Only the offline driver exists yet; a device
 * output is refused until the devices workstream lands (docs/design.md 8.3).
 */
export function openEngine(options: { output: { offline: true } }): Promise<OfflineEngine>;
export function openEngine(options: OpenEngineOptions): Promise<Engine>;
export async function openEngine(options: OpenEngineOptions): Promise<Engine> {
  if (!("offline" in options.output)) {
    throw new Error("device output is not built yet; open with { output: { offline: true } }");
  }
  const native = new NativeOfflineEngine();
  const engine: OfflineEngine = {
    capabilities: new Set<Capability>(),
    output: outputInfo(native.output),
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
  return engine;
}

function outputInfo(info: NativeOutputInfo): OutputInfo {
  if (info.sampleRate !== 48000) {
    throw new Error(`the engine runs at 48 kHz; the addon reports ${info.sampleRate}`);
  }
  return { sampleRate: 48000, periodFrames: info.periodFrames, latencyMs: info.latencyMs };
}
