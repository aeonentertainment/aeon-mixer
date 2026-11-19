import { FADE_BLOCKS } from "../src/contract/offline-clock.ts";
import type { Mixer, OfflineEngine } from "../src/index.ts";

/** What `stats()` reads for a slot that has heard nothing. */
export const SILENT_STATS = {
  pushed: 0,
  lost: 0,
  late: 0,
  lateAtOpening: 0,
  concealed: 0,
  resyncs: 0,
  jitterMs: 0,
  correctorDelayMs: 0,
  correctorMaxDelayMs: 0,
  echo: null,
  playoutDelayMs: 0,
  howlCatches: 0,
  tone: null,
};

/** Destroys a live mixer, pumping the offline clock through the swap fade so the acks arrive. */
export async function destroyLive(engine: OfflineEngine, mixer: Mixer) {
  const destroyed = mixer.destroy();
  engine.pump(FADE_BLOCKS);
  await destroyed;
}

/** The left channel of interleaved stereo. */
export function left(interleaved: Float32Array) {
  return Array.from(interleaved.filter((_, i) => i % 2 === 0));
}
