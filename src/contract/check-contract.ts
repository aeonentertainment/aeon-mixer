import { enginePromises } from "./engine-promises.ts";
import { closeOffline, messageOf, within } from "./offline-clock.ts";
import { senderPromises } from "./sender-promises.ts";
import { slotPromises } from "./slot-promises.ts";
import type {
  ContractEngine,
  ContractOptions,
  ContractPromise,
  ContractSubject,
  Trial,
} from "./contract-subject.ts";

const PROMISES = [...enginePromises, ...slotPromises, ...senderPromises];
/** Longer than any check takes, so only a call that never settles reaches it. */
const CHECK_MS = 10_000;

export interface ContractReport {
  /** Each promise the subject broke, with what the suite found. Empty when it keeps them all. */
  broken: Array<{ promise: string; found: string }>;
  /** Each promise this run could not check, and why. */
  unchecked: Array<{ promise: string; why: string }>;
}

/**
 * Checks `subject` against the promises of docs/design.md 7.2 that a host relies on, one at a
 * time on fresh offline engines, and reports each one it broke in a sentence. A host runs it
 * against this package's module and against any stand-in its tests drive in the package's place:
 * `expect((await checkContract(mixer)).broken).toEqual([])`.
 */
export async function checkContract<E extends ContractEngine>(
  subject: ContractSubject<E>,
  options: ContractOptions<E> = {},
): Promise<ContractReport> {
  const report: ContractReport = { broken: [], unchecked: [] };
  for (const { promise, check } of PROMISES) {
    // oxlint-disable-next-line no-await-in-loop -- one promise at a time, each on engines of its own
    const verdict = await run(check, subject, options);
    if (verdict && "found" in verdict) report.broken.push({ promise, found: verdict.found });
    else if (verdict) report.unchecked.push({ promise, why: verdict.unchecked });
  }
  return report;
}

/**
 * Runs one check and closes every engine it opened. A throw, a call that never settles or a close
 * that never settles is what it found.
 */
async function run<E extends ContractEngine>(
  check: ContractPromise["check"],
  subject: ContractSubject<E>,
  { moveOutput }: ContractOptions<E>,
) {
  const opened: E[] = [];
  const trial: Trial = {
    async open(options = {}) {
      const engine = await subject.openEngine({ ...options, output: { offline: true } });
      opened.push(engine);
      return { engine, move: moveOutput && (() => moveOutput(engine)) };
    },
    outputDevices: () => subject.outputDevices(),
  };
  const outcome = await within(check(trial), CHECK_MS);
  const closed = await within(Promise.all(opened.map(closeOffline)), CHECK_MS);
  if (outcome === "pending")
    return { found: `a call it awaited never settled in ${CHECK_MS / 1000} s` };
  if ("error" in outcome) return { found: `it threw: ${messageOf(outcome.error)}` };
  if (!outcome.value && closed === "pending") return { found: "close never settled" };
  return outcome.value;
}
