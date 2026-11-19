import type { MelodyGuide, SlotEffect, SlotSettings, StemKind } from "../types.ts";
import type { BundleOutput } from "./bundle-manifest.ts";

/**
 * One line of a session bundle's `events.jsonl`, less the `frame` and `us` every line carries:
 * docs/design.md 8.11. A call's line holds its arguments as the host gave them, so a parameter
 * the call left out is left out of the line (8.12). Every line is written in the turn that makes
 * its call, so the file holds the calls in the order they were made; a command's ack writes its
 * own `ack` line. `level` and `pitch` are not here: a replay makes them again.
 */
export type BundleEvent =
  | { type: "setSlot"; index: number; settings: Partial<SlotSettings> }
  | { type: "setSlotEffects"; index: number; effects: ReadonlyArray<SlotEffect> }
  | { type: "setSlotGuide"; index: number; guide: MelodyGuide | null; guideOffsetMs?: number }
  | { type: "resetSlot"; index: number }
  | CommandCall
  /** A command's ack, as it arrives: the backing position at the block it took effect. */
  | { type: "ack"; call: CommandCall["type"]; positionMs: number }
  | { type: "setStemGain"; kind: StemKind; gain: number }
  | { type: "outputChanged"; output: BundleOutput; gapMs: number; heldMs: number }
  /** The stems are in the graph: their length in frames, at the armed speed. */
  | { type: "backingRendered"; frames: number }
  | { type: "backingEnded" }
  | { type: "error"; message: string }
  /** Written by `destroy`, last: a bundle without it was cut short. */
  | { type: "end" };

/** A call that sends the engine a command and resolves on its ack. */
export type CommandCall =
  | { type: "start" }
  | { type: "seek"; toMs: number }
  | { type: "pause" | "resume" }
  | { type: "setBackingKey"; semitones: number };
