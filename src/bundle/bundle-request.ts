import { createHash } from "node:crypto";
import { createReadStream, existsSync } from "node:fs";
import { setImmediate as nextTurn } from "node:timers/promises";
import { type BackingConfig, backingSettings } from "../backing.ts";
import type { SettingsProfile } from "../profile/settings-profile.ts";
import type { BackingStem, MixerConfig } from "../types.ts";
import type { BundleBacking } from "./bundle-manifest.ts";

/** What a mixer needs to start its bundle: where, and the backing as its manifest names it. */
export interface BundleRequest {
  dir: string;
  backing: BundleBacking | null;
}

/**
 * `config.bundle` checked, with each stem hashed, before the mixer exists: a directory that is
 * already there, a bad backing or a stem that cannot be read rejects `createMixer` with no port
 * bound and nothing written.
 */
export async function bundleRequest(
  { dir }: NonNullable<MixerConfig["bundle"]>,
  backing: BackingConfig | undefined,
  defaults: SettingsProfile["backing"]["stemGains"],
): Promise<BundleRequest> {
  if (typeof dir !== "string" || dir === "") {
    throw new TypeError("bundle.dir must name a directory");
  }
  if (existsSync(dir)) throw new Error(`bundle.dir already exists: ${dir}`);
  if (backing === undefined) return { dir, backing: null };
  // The check the mixer makes, made first, so a bad config is refused by name and not by a read
  backingSettings(backing, defaults);
  return {
    dir,
    backing: {
      stems: await Promise.all(
        backing.stems.map(async (stem) => ({ kind: stem.kind, sha256: await sha256(stem) })),
      ),
      key: backing.key,
      speed: backing.speed,
      stemGains: { ...backing.stemGains },
      stopAtMs: backing.stopAtMs ?? null,
    },
  };
}

/** 1 MiB hashes in under half a millisecond, so the control thread answers between slices. */
const SLICE_BYTES = 1 << 20;

/**
 * A path read as a stream and bytes hashed a slice a turn, so a song's worth of stem never holds
 * the control thread. The same file by either has the same hash.
 */
async function sha256({ path, bytes }: BackingStem) {
  const hash = createHash("sha256");
  if (bytes === undefined) {
    for await (const chunk of createReadStream(path)) hash.update(chunk);
  } else {
    for (let at = 0; at < bytes.length; at += SLICE_BYTES) {
      // oxlint-disable-next-line no-await-in-loop -- each slice waits for the next turn on purpose
      if (at > 0) await nextTurn();
      hash.update(bytes.subarray(at, at + SLICE_BYTES));
    }
  }
  return hash.digest("hex");
}
