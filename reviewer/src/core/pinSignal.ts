// A tiny shared signal so the file-jump pin loop (useFileNavigation) and the
// header collapse (useChromeCollapse) agree: while a jump is being held, the
// header must stay shown and the collapse must ignore the loop's own scrolls.

let pinning = false;
const subscribers = new Set<(value: boolean) => void>();

export function setPinning(value: boolean): void {
  if (value === pinning) return;
  pinning = value;
  for (const fn of subscribers) fn(value);
}

export function isPinning(): boolean {
  return pinning;
}

export function subscribePinning(fn: (value: boolean) => void): () => void {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}
