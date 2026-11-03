type Handler = (...args: never[]) => void;

/** Typed listeners for one object's events. `on` returns the unsubscribe. */
export class Emitter<Events extends Record<keyof Events, Handler>> {
  readonly #listeners = new Map<keyof Events, Set<Handler>>();

  on<K extends keyof Events>(event: K, listener: Events[K]) {
    let listeners = this.#listeners.get(event);
    if (!listeners) {
      listeners = new Set();
      this.#listeners.set(event, listeners);
    }
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }

  emit<K extends keyof Events>(event: K, ...args: Parameters<Events[K]>) {
    for (const listener of this.#listeners.get(event) ?? []) {
      (listener as (...args: Parameters<Events[K]>) => void)(...args);
    }
  }
}
