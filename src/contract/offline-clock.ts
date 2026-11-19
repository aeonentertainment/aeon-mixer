import { setTimeout as delay } from "node:timers/promises";
import type { ContractEngine, ContractMixer } from "./contract-subject.ts";

export const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
const FADE_BLOCKS = 3;

/** Starts `mixer` on the offline clock, pumping through the swap's fade so its acks arrive. */
export async function startLive(engine: ContractEngine, mixer: ContractMixer) {
  const started = mixer.start();
  engine.pump(FADE_BLOCKS);
  return started;
}

/**
 * Closes `engine`, pumping it through a live mixer's fade so the close waits on no ack. An engine
 * its check closed already has no clock left to pump, and closes again at once.
 */
export async function closeOffline(engine: ContractEngine) {
  const closed = engine.close();
  try {
    engine.pump(FADE_BLOCKS);
  } catch {
    // Closed by the check
  }
  await closed;
}

/**
 * Pumps a block a millisecond until `done` holds, false once `ms` have passed. A datagram on
 * loopback is read on the network thread, which leaves nothing to await: the clock has to run
 * for the slot to drain it, and running it near real time keeps a sender's second of quiet a
 * second. Only a broken promise waits out the deadline.
 */
export async function pumpUntil(engine: ContractEngine, done: () => boolean, ms = 1000) {
  const deadline = Date.now() + ms;
  while (!done()) {
    if (Date.now() > deadline) return false;
    // oxlint-disable-next-line no-await-in-loop -- a block a millisecond, so the clock keeps near real time
    await delay(1);
    engine.pump(1);
  }
  return true;
}

/** How `promise` settles within `ms`, or "pending" when it has not: a call that never settles is a broken promise, not a hung suite. */
export async function within<T>(
  promise: Promise<T>,
  ms: number,
): Promise<{ value: T } | { error: unknown } | "pending"> {
  const controller = new AbortController();
  const timeout = delay(ms, "pending" as const, { signal: controller.signal }).catch(
    () => "pending" as const,
  );
  try {
    return await Promise.race([
      promise.then(
        (value) => ({ value }),
        (error) => ({ error }),
      ),
      timeout,
    ]);
  } finally {
    controller.abort();
  }
}

export function messageOf(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
