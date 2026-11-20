import { LocalSender, widerAddress } from "./local-sender.ts";
import { pumpUntil, startLive } from "./offline-clock.ts";
import type {
  ContractEngine,
  ContractMixer,
  ContractPromise,
  Trial,
  Verdict,
} from "./contract-subject.ts";

interface HeldSlot {
  engine: ContractEngine;
  mixer: ContractMixer;
  port: number;
  /** The sender the slot is playing. */
  held: LocalSender;
  /** Another address, a phone that has not sent yet. */
  next: LocalSender;
  ingest: () => { pushed: number; resyncs: number };
}

/**
 * Runs `body` on a live `l16` slot that has taken one packet from `held`: decision 0028's lock.
 * A refused packet counts in no stat, so each check ends on a packet that counts only when the
 * promise holds, or on a new stream's resync, which comes after everything sent before it.
 * `LocalSender` says why the two senders share an SSRC.
 */
async function withHeldSlot(trial: Trial, body: (slot: HeldSlot) => Promise<Verdict>) {
  const { engine } = await trial.open();
  const mixer = await engine.createMixer({ slots: [{ kind: "l16" }] });
  const port = mixer.slots[0]?.port;
  if (typeof port !== "number") return { found: `an l16 slot had the port ${String(port)}` };
  await startLive(engine, mixer);
  const held = await LocalSender.open();
  const next = await LocalSender.open();
  const ingest = () => {
    const { pushed = 0, resyncs = 0 } = mixer.stats().slots[0] ?? {};
    return { pushed, resyncs };
  };
  try {
    await held.send(port);
    if (!(await pumpUntil(engine, () => ingest().pushed === 1))) {
      return { found: "an idle slot never took a sender's packet" };
    }
    return await body({ engine, mixer, port, held, next, ingest });
  } finally {
    held.close();
    next.close();
  }
}

/** Resets the slot and runs a block, so the reset is in before the next packet lands. */
function reset({ engine, mixer }: HeldSlot) {
  mixer.resetSlot(0);
  engine.pump(1);
}

export const senderPromises: ReadonlyArray<ContractPromise> = [
  {
    promise:
      "An `rtp` slot takes datagrams from this machine only, unless its `listen` is `every-interface`.",
    needs: ["sockets"],
    async check(trial) {
      const { engine } = await trial.open();
      const mixer = await engine.createMixer({
        slots: [{ kind: "rtp" }, { kind: "rtp", listen: "every-interface" }],
      });
      const [loopback, every] = mixer.slots.map((slot) => slot.port);
      if (typeof loopback !== "number" || typeof every !== "number") {
        return { found: `two rtp slots had the ports ${String(loopback)} and ${String(every)}` };
      }
      const address = widerAddress();
      if (address === undefined) {
        return { unchecked: "this machine has no IPv4 address but loopback to send from" };
      }
      await startLive(engine, mixer);
      const pushed = () => mixer.stats().slots.map((slot) => slot.pushed);
      const wide = await LocalSender.open(address);
      const near = await LocalSender.open();
      try {
        // The kernel has no socket for the first datagram, so it is gone before the second lands
        await wide.send(loopback);
        await wide.send(every);
        if (!(await pumpUntil(engine, () => pushed()[1] === 1))) {
          return {
            found: `a slot whose listen is every-interface never heard a datagram sent to ${address}`,
          };
        }
        if (pushed()[0] !== 0) {
          return { found: `a slot left at its default heard a datagram sent to ${address}` };
        }
        await near.send(loopback);
        if (await pumpUntil(engine, () => pushed()[0] === 1)) return undefined;
        return { found: "a slot left at its default never heard a datagram sent on loopback" };
      } finally {
        wide.close();
        near.close();
      }
    },
  },
  {
    promise: "A slot playing one sender refuses every other address.",
    needs: ["sockets"],
    check: (trial) =>
      withHeldSlot(trial, async ({ engine, port, held, next, ingest }) => {
        await next.send(port);
        await held.send(port);
        held.restart();
        await held.send(port);
        await pumpUntil(engine, () => ingest().resyncs >= 1);
        const { pushed, resyncs } = ingest();
        if (pushed === 3 && resyncs === 1) return undefined;
        return {
          found: `the slot counted ${pushed} of the 4 packets sent and ${resyncs} new streams, where only its sender's 3, starting over once, should count: another address got in`,
        };
      }),
  },
  {
    promise: "After `resetSlot`, a slot takes a new sender at once.",
    needs: ["sockets"],
    check: (trial) =>
      withHeldSlot(trial, async (slot) => {
        reset(slot);
        await slot.next.send(slot.port);
        if (await pumpUntil(slot.engine, () => slot.ingest().pushed === 2)) return undefined;
        return { found: "after `resetSlot`, the slot still refused a new sender" };
      }),
  },
  {
    promise: "After `resetSlot`, a slot refuses the sender it released.",
    needs: ["sockets"],
    check: (trial) =>
      withHeldSlot(trial, async (slot) => {
        const { engine, port, held, next, ingest } = slot;
        reset(slot);
        await held.send(port);
        await next.send(port);
        await next.send(port);
        next.restart();
        await next.send(port);
        await pumpUntil(engine, () => ingest().resyncs >= 2);
        const { pushed, resyncs } = ingest();
        if (pushed === 4 && resyncs === 2) return undefined;
        return {
          found: `after \`resetSlot\`, the slot counted ${pushed - 1} of the 4 packets sent and ${resyncs} new streams, where only the new sender's 3, on 2 streams, should count: it took back the sender it released`,
        };
      }),
  },
];
