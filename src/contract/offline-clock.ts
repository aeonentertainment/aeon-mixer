import { setTimeout as delay } from "node:timers/promises";
import type { ContractEngine, ContractMixer } from "./contract-subject.ts";

export const BLOCK = 128;
/** The swap fade is 5 ms, under two blocks; the third block is clean. */
export const FADE_BLOCKS = 3;

/** How long a call the suite awaits may take to settle before it counts as never settling. */
export const SETTLE_MS = 2000;

/**
 * Starts `mixer` on the offline clock, pumping through the swap's fade so its acks arrive, and
 * returns what the start resolved. The start is heard from the moment it is made, so a `pump`
 * that throws leaves no rejection unheard.
 */
export async function startLive(
  engine: Pick<ContractEngine, "pump">,
  mixer: Pick<ContractMixer, "start">,
) {
  const started = within(mixer.start(), SETTLE_MS);
  engine.pump(FADE_BLOCKS);
  const outcome = await started;
  if (outcome === "pending") throw new Error("start never settled");
  if ("error" in outcome) throw new Error(`start rejected: ${messageOf(outcome.error)}`);
  return outcome.value;
}

/**
 * Resolves once `mixer`'s backing has rendered and its stems are in the graph, with every
 * progress fraction it reported on the way.
 */
export function rendered(mixer: Pick<ContractMixer, "on">) {
  const fractions: number[] = [];
  return new Promise<number[]>((resolve) => {
    mixer.on("renderProgress", (fraction) => {
      fractions.push(fraction);
      if (fraction === 1) resolve(fractions);
    });
  });
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
 * Pumps a block a millisecond at most until `done` holds, false once `ms` have passed. A datagram
 * on loopback is read on the network thread, which leaves nothing to await: the clock has to run
 * for the slot to drain it. A 2.67 ms block per wait of 1 ms or more runs the engine at most about
 * 2.7 times wall time, so a sender's second of quiet lasts at least 370 ms here, far longer than a
 * datagram takes to reach the slot. Only a broken promise waits out the deadline.
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
