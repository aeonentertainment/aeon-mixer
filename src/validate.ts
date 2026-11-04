// Range checks at the edge, so nothing out of range crosses to the native side.

import { isIP } from "node:net";
import type { RtpReturnInput } from "./native.ts";
import type { IngestKind, MixerConfig } from "./types.ts";

const INGEST_KINDS: ReadonlyArray<IngestKind> = ["rtp", "l16", "push"];

/** A kind as a JavaScript caller may have spelt it, so an unknown one never crosses. */
export function ingestKind(name: string, value: string) {
  const kind = INGEST_KINDS.find((known) => known === value);
  if (kind === undefined) {
    throw new RangeError(`${name} must be rtp, l16 or push, got ${value}`);
  }
  return kind;
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

export function finiteIn(name: string, value: number, min: number, max: number) {
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
