// The one place milliseconds become frames and back: 48 frames a millisecond at 48 kHz.

const FRAMES_PER_MS = 48;

export function msToFrames(ms: number): number {
  return Math.round(ms * FRAMES_PER_MS);
}

export function framesToMs(frames: number): number {
  return frames / FRAMES_PER_MS;
}
