/** Freezes `value` and every object in it, so a profile in force cannot change under its engine. */
export function deepFreeze<T extends object>(value: T): T {
  const children: unknown[] = Object.values(value);
  for (const child of children) {
    if (typeof child === "object" && child !== null) deepFreeze(child);
  }
  return Object.freeze(value);
}
