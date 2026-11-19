import type { IngestKind, OutputInfo, StemKind } from "../types.ts";

/**
 * A session bundle's `manifest.json`, format 1: docs/design.md 8.11. It names the output and
 * hashes the stems, and holds no path on the host's disk and no device id.
 */
export interface BundleManifest {
  format: 1;
  /** This package's version. */
  package: string;
  /** Where it ran, as Node names it: `darwin-arm64`. */
  platform: string;
  output: BundleOutput;
  /** The bundle's clock as the mixer was created. */
  frame: number;
  us: number;
  /** A `push` or `capture` slot has no packets to record, and replays as silence. */
  slots: ReadonlyArray<{ index: number; kind: IngestKind; recorded: boolean }>;
  backing: BundleBacking | null;
}

/** An output as a bundle names it: `name` is null on the timer and offline clocks. */
export interface BundleOutput {
  sampleRate: 48000;
  periodFrames: number;
  latencyMs: number;
  name: string | null;
}

/**
 * The backing as the host asked for it. A stem is named by the SHA-256 of its file, and a stem
 * gain the config left out is left out here, so a replay under another profile takes that
 * profile's (8.12).
 */
export interface BundleBacking {
  stems: ReadonlyArray<{ kind: StemKind; sha256: string }>;
  key: number;
  speed: number;
  stemGains: Partial<Record<StemKind, number>>;
  stopAtMs: number | null;
}

export function bundleOutput({ sampleRate, periodFrames, latencyMs, device }: OutputInfo) {
  return { sampleRate, periodFrames, latencyMs, name: device?.name ?? null } satisfies BundleOutput;
}
