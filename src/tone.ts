// A tone stage's settings, checked before they cross: each value's range here, and the band count
// and the delay and lift caps by the engine's own check, so the filters judged are the filters
// that run.

import { nativeCheckTone, type ToneInput } from "./native.ts";
import type { SlotEffect, ToneBand } from "./types.ts";
import { finiteIn } from "./validate.ts";

const TONE_BAND_KINDS: ReadonlySet<string> = new Set<ToneBand["kind"]>([
  "peak",
  "low-shelf",
  "high-shelf",
]);

/** The stage off with nothing asked: a slot's as it starts. */
export function toneOff(): ToneInput {
  return { enabled: false, bands: [], plosiveDb: 0, airDb: 0, deEssDb: 0 };
}

/** Throws naming the field out of range, or the frequency and figure a cap refuses. */
export function toneInput(effect: Extract<SlotEffect, { kind: "tone" }>): ToneInput {
  const { bands = [], plosiveDb = 0, airDb = 0, deEssDb = 0 } = effect;
  if (bands === "match") {
    throw new RangeError(
      'bands "match" has no target curve to aim at yet; pass the bands for the mic, or none',
    );
  }
  const input: ToneInput = {
    enabled: effect.enabled,
    bands: bands.map((band, i) => toneBand(`bands[${i}]`, band)),
    plosiveDb: finiteIn("plosiveDb", plosiveDb, 0, 12),
    airDb: finiteIn("airDb", airDb, 0, 6),
    deEssDb: finiteIn("deEssDb", deEssDb, 0, 12),
  };
  try {
    nativeCheckTone(input);
  } catch (error) {
    throw new RangeError(error instanceof Error ? error.message : String(error));
  }
  return input;
}

function toneBand(name: string, band: ToneBand): ToneInput["bands"][number] {
  if (!TONE_BAND_KINDS.has(band.kind)) {
    throw new RangeError(`${name}.kind must be peak, low-shelf or high-shelf, got ${band.kind}`);
  }
  const hz = finiteIn(`${name}.hz`, band.hz, 80, 16_000);
  const gainDb = finiteIn(`${name}.gainDb`, band.gainDb, -12, 6);
  return band.kind === "peak"
    ? { kind: "peak", hz, gainDb, q: finiteIn(`${name}.q`, band.q, 0.3, 8) }
    : { kind: band.kind, hz, gainDb };
}
