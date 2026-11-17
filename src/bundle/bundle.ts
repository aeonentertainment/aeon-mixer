import { appendFileSync, closeSync, mkdirSync, openSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import packageJson from "../../package.json" with { type: "json" };
import type { ClockReading } from "../native.ts";
import type { SettingsProfile } from "../profile/settings-profile.ts";
import type { IngestKind, OutputInfo, SlotStats } from "../types.ts";
import type { BundleEvent } from "./bundle-event.ts";
import { type BundleBacking, type BundleManifest, bundleOutput } from "./bundle-manifest.ts";

const STATS_INTERVAL_MS = 1000;
/** A bundle holds every singer's voice in the song, so on Unix only its owner reads it. */
const OWNER_ONLY = { dir: 0o700, file: 0o600 };

/** What a bundle reads from the mixer it records. */
export interface BundleSource {
  readonly profile: SettingsProfile;
  readonly output: OutputInfo;
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind }>;
  /** The frames rendered and the wall clock the pcaps stamp arrivals with, read in one call. */
  clock(): ClockReading;
  /** Records a slot's datagrams to a new pcap at `path`, each sender hidden behind a port of its own. */
  record(slot: number, path: string): void;
  /** `stats()` and the backing's position as the engine last reported it, null before it has. */
  stats(): {
    positionMs: number | null;
    slots: ReadonlyArray<SlotStats>;
    underruns: number;
    rtpReturn: { droppedBlocks: number } | null;
  };
}

/**
 * One mixer's session bundle, written from `createMixer` to `destroy`: docs/design.md 8.11. The
 * manifest, the profile and every slot's pcap start here; `events.jsonl` and `stats.jsonl` grow
 * a line at a time, each one write on the control thread, so a process that dies leaves files
 * that read to their last line. A write that fails ends the bundle and never the song: the
 * bundle then has no `end` line, which is how one cut short reads.
 */
export class Bundle {
  readonly #clock: () => ClockReading;
  readonly #events: number;
  readonly #stats: number;
  readonly #timer: NodeJS.Timeout;
  #open = true;

  /** Throws when `dir` exists, or when anything in it cannot be made. */
  constructor(dir: string, backing: BundleBacking | null, source: BundleSource) {
    this.#clock = () => source.clock();
    mkdirSync(dir, { mode: OWNER_ONLY.dir });
    mkdirSync(join(dir, "slots"), { mode: OWNER_ONLY.dir });
    mkdirSync(join(dir, "host"), { mode: OWNER_ONLY.dir });
    const slots = source.slots.map(({ index, kind }) => ({
      index,
      kind,
      recorded: kind === "rtp" || kind === "l16",
    }));
    for (const { index, recorded } of slots) {
      if (recorded) source.record(index, join(dir, "slots", `${index}.pcap`));
    }
    const manifest: BundleManifest = {
      format: 1,
      package: packageJson.version,
      platform: `${process.platform}-${process.arch}`,
      output: bundleOutput(source.output),
      ...source.clock(),
      slots,
      backing,
    };
    writeJson(join(dir, "manifest.json"), manifest);
    writeJson(join(dir, "profile.json"), source.profile);
    this.#events = openSync(join(dir, "events.jsonl"), "ax", OWNER_ONLY.file);
    this.#stats = openSync(join(dir, "stats.jsonl"), "ax", OWNER_ONLY.file);
    this.#timer = setInterval(() => {
      this.#append(this.#stats, { ...this.#clock(), ...source.stats() });
    }, STATS_INTERVAL_MS);
    // A bundle keeps no process alive
    this.#timer.unref();
  }

  /**
   * Reads the clock and returns what writes one event line stamped with that reading. A call
   * takes its stamp before it changes anything, so the block that takes the change up starts at
   * or after the line's `frame`, and writes the line in the same turn, once the change is made
   * or its command sent, so the lines are in the order their calls were made.
   */
  stamp() {
    const at = this.#clock();
    return ({ type, ...fields }: BundleEvent) => {
      this.#append(this.#events, { type, ...at, ...fields });
    };
  }

  /** An event line stamped now: what the engine reported. */
  event(event: BundleEvent) {
    this.stamp()(event);
  }

  /** Writes `end` and closes the files. Nothing is written after it. */
  end() {
    this.event({ type: "end" });
    this.#close();
  }

  #append(file: number, line: object) {
    if (!this.#open) return;
    try {
      appendFileSync(file, `${JSON.stringify(line)}\n`);
    } catch {
      this.#close();
    }
  }

  #close() {
    if (!this.#open) return;
    this.#open = false;
    clearInterval(this.#timer);
    for (const file of [this.#events, this.#stats]) {
      try {
        closeSync(file);
      } catch {
        // The bundle is over either way
      }
    }
  }
}

function writeJson(path: string, value: object) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx", mode: OWNER_ONLY.file });
}
