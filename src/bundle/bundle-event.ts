import type { MelodyGuide, SlotEffect, SlotSettings, StemKind } from "../types.ts";
import type { BundleOutput } from "./bundle-manifest.ts";

/**
 * One line of a session bundle's `events.jsonl`, less the `frame` and `us` every line carries:
 * docs/design.md 8.11. A call's line holds its arguments as the host gave them, so a parameter
 * the call left out is left out of the line (8.12). A command's line is written when its ack
 * arrives, with the backing position the ack gave. `level` and `pitch` are not here: a replay
 * makes them again.
 */
export type BundleEvent =
  | { type: "setSlot"; index: number; settings: Partial<SlotSettings> }
  | { type: "setSlotEffects"; index: number; effects: ReadonlyArray<SlotEffect> }
  | { type: "setSlotGuide"; index: number; guide: MelodyGuide | null; guideOffsetMs?: number }
  | { type: "resetSlot"; index: number }
  | { type: "start"; positionMs: number }
  | { type: "seek"; toMs: number; positionMs: number }
  | { type: "pause"; positionMs: number }
  | { type: "resume"; positionMs: number }
  | { type: "setBackingKey"; semitones: number; positionMs: number }
  | { type: "setStemGain"; kind: StemKind; gain: number }
  | { type: "outputChanged"; output: BundleOutput; gapMs: number }
  /** The stems are in the graph: their length in frames, at the armed speed. */
  | { type: "backingRendered"; frames: number }
  | { type: "backingEnded" }
  | { type: "error"; message: string }
  /** Written by `destroy`, last: a bundle without it was cut short. */
  | { type: "end" };
