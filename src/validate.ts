// Range checks at the edge, so nothing out of range crosses to the native side.

export function finiteIn(name: string, value: number, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${name} must be a finite number, got ${String(value)}`);
  }
  if (value < min || value > max) {
    throw new RangeError(`${name} must be between ${min} and ${max}, got ${value}`);
  }
  return value;
}

export function integerIn(name: string, value: number, min: number, max: number): number {
  if (!Number.isInteger(value)) {
    throw new TypeError(`${name} must be an integer, got ${String(value)}`);
  }
  return finiteIn(name, value, min, max);
}

export function pcm(name: string, value: Float32Array, channels: number): Float32Array {
  if (!(value instanceof Float32Array) || value.length === 0) {
    throw new TypeError(`${name} must be a non-empty Float32Array`);
  }
  if (value.length % channels !== 0) {
    throw new RangeError(`${name} must hold whole frames of ${channels} channels`);
  }
  return value;
}
