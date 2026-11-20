// Range checks at the edge, so nothing out of range crosses to the native side.

import { isIP } from "node:net";
import type { RtpReturnInput, SlotInput } from "./native.ts";
import type { IngestKind, MixerConfig, SlotConfig, SlotListen } from "./types.ts";

const INGEST_KINDS: ReadonlyArray<IngestKind> = ["rtp", "l16", "push", "capture"];
const LISTENS: ReadonlyArray<SlotListen> = ["loopback", "every-interface"];

/** A kind as a JavaScript caller may have spelt it, so an unknown one never crosses. */
export function ingestKind(name: string, value: string) {
  const kind = INGEST_KINDS.find((known) => known === value);
  if (kind === undefined) {
    throw new RangeError(`${name} must be rtp, l16, push or capture, got ${value}`);
  }
  return kind;
}

/**
 * A slot as the addon takes it: its kind, for an `rtp` slot where it listens, and for a `capture`
 * slot its device and channel. `listen` belongs to an `rtp` slot alone, so it is refused on any
 * other, where it would bind nothing a host asked for.
 */
export function slotInput(name: string, slot: SlotConfig): SlotInput & { kind: IngestKind } {
  const kind = ingestKind(`${name}.kind`, slot.kind);
  // A JavaScript caller may put `listen` on any kind, so it is looked for on every one
  const listen = "listen" in slot ? slot.listen : undefined;
  if (slot.kind === "rtp") {
    if (listen === undefined) return { kind };
    const known = LISTENS.find((where) => where === listen);
    if (known === undefined) {
      throw new RangeError(`${name}.listen must be loopback or every-interface, got ${listen}`);
    }
    return { kind, listen: known };
  }
  if (listen !== undefined) {
    throw new TypeError(`${name}.listen is for an rtp slot, not ${kind}`);
  }
  if (slot.kind !== "capture") return { kind };
  const { device, channel = 0 } = slot;
  if (device !== undefined && (typeof device !== "string" || device === "")) {
    throw new TypeError(`${name}.device must be an id from inputDevices()`);
  }
  return {
    kind,
    channel: integerIn(`${name}.channel`, channel, 0, 255),
    ...(device === undefined ? {} : { device }),
  };
}

/**
 * Where the mix goes: an IP literal, since resolving a name would block the control thread, and a
 * dynamic RTP payload type, since Opus has no static one.
 */
export function rtpReturn(value: NonNullable<MixerConfig["rtpReturn"]>): RtpReturnInput {
  if (isIP(value.host) === 0) {
    throw new TypeError(`rtpReturn.host must be an IP address, got ${value.host}`);
  }
  return {
    host: value.host,
    port: integerIn("rtpReturn.port", value.port, 1, 65535),
    payloadType: integerIn("rtpReturn.payloadType", value.payloadType, 96, 127),
    ssrc: integerIn("rtpReturn.ssrc", value.ssrc, 0, 0xffff_ffff),
  };
}

/**
 * The range each setting takes wherever it is set: a setter, a mixer's config or a settings
 * profile.
 */
export const RANGES = {
  gain: [0, 4],
  pan: [-1, 1],
  strength: [0, 1],
  retuneMs: [0, 1000],
  roomSize: [0, 1],
  mix: [0, 1],
  formantSemitones: [-12, 12],
  stemGain: [0, 4],
  // A pinned delay can never rise, so under 5 ms, two blocks, ordinary jitter would be all late
  // packets for good. A slot left to choose goes as low as 3.67 ms, because it rises when packets
  // come late. 100 ms stays inside the 64 packets the buffer holds, 160 ms of the shortest frame
  playoutDelayMs: [5, 100],
} as const;

export function outputPeriod(name: string, value: unknown) {
  if (value !== 64 && value !== 128) {
    throw new RangeError(`${name} must be 64 or 128, got ${String(value)}`);
  }
  return value;
}

/** `value` is `unknown` so a profile read from JSON is checked by the code the setters use. */
export function finiteIn(name: string, value: unknown, min: number, max: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${name} must be a finite number, got ${String(value)}`);
  }
  if (value < min || value > max) {
    throw new RangeError(`${name} must be between ${min} and ${max}, got ${value}`);
  }
  return value;
}

export function integerIn(name: string, value: number, min: number, max: number) {
  if (!Number.isInteger(value)) {
    throw new TypeError(`${name} must be an integer, got ${String(value)}`);
  }
  return finiteIn(name, value, min, max);
}

export function pcm(name: string, value: Float32Array, channels: number) {
  if (!(value instanceof Float32Array) || value.length === 0) {
    throw new TypeError(`${name} must be a non-empty Float32Array`);
  }
  if (value.length % channels !== 0) {
    throw new RangeError(`${name} must hold whole frames of ${channels} channels`);
  }
  return value;
}
