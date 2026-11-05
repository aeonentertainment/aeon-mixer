/** How long a command may go unacknowledged before its promise rejects (docs/design.md 8.8). */
export const ACK_TIMEOUT_MS = 1000;

interface Pending {
  resolve(positionFrames: number): void;
  reject(error: Error): void;
  timer: NodeJS.Timeout;
}

/** Commands in flight, each resolved by the ack that carries its id. */
export class Acks {
  readonly #pending = new Map<number, Pending>();

  /**
   * `state` describes the engine for the rejection when no ack comes, and `onTimeout` hears the
   * same error; a command rejected any other way never reaches it.
   */
  wait(id: number, state: () => string, onTimeout?: (error: Error) => void): Promise<number> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        const error = new Error(
          `command ${id} was not acknowledged within ${ACK_TIMEOUT_MS} ms; the engine is ${state()}`,
        );
        reject(error);
        onTimeout?.(error);
      }, ACK_TIMEOUT_MS);
      timer.unref();
      this.#pending.set(id, { resolve, reject, timer });
    });
  }

  resolve(id: number, positionFrames: number) {
    const pending = this.#pending.get(id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.#pending.delete(id);
    pending.resolve(positionFrames);
  }

  rejectAll(error: Error) {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.#pending.clear();
  }
}
