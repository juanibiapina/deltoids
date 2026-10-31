// Find hunks relative to the reading position: the phone layout's bottom
// bar and the desktop z/x keys act on the hunk at the top of the scroller.

function hunks(scroller: HTMLElement): HTMLElement[] {
  return [...scroller.querySelectorAll<HTMLElement>(".file:not([hidden]) .hunk[data-hunk]")];
}

// The height a sticky file header covers at the top of the scroller.
function topInset(scroller: HTMLElement): number {
  const box = scroller.getBoundingClientRect();
  for (const head of scroller.querySelectorAll<HTMLElement>(".file:not([hidden]) .file-head")) {
    if (getComputedStyle(head).position !== "sticky") return 0;
    const rect = head.getBoundingClientRect();
    if (rect.bottom > box.top) return rect.bottom - box.top;
  }
  return 0;
}

function readingTop(scroller: HTMLElement): number {
  return scroller.getBoundingClientRect().top + topInset(scroller);
}

// The hunk under the reading position, else the first one below it.
export function hunkAtTop(scroller: HTMLElement): HTMLElement | null {
  const top = readingTop(scroller) + 1;
  return hunks(scroller).find((h) => h.getBoundingClientRect().bottom > top) ?? null;
}

// Scroll the next (1) or previous (-1) hunk's start to the reading position.
// Returns false at either end.
export function scrollToHunk(scroller: HTMLElement, direction: 1 | -1): boolean {
  const top = readingTop(scroller);
  const all = hunks(scroller);
  const target =
    direction === 1
      ? all.find((h) => h.getBoundingClientRect().top > top + 1)
      : all.filter((h) => h.getBoundingClientRect().top < top - 1).pop();
  if (!target) return false;
  scroller.scrollTop += target.getBoundingClientRect().top - top;
  return true;
}

// Grow or shrink the context of the hunk at the reading position.
export function reshapeHunkAtTop(scroller: HTMLElement, op: "expand" | "shrink"): boolean {
  const button = hunkAtTop(scroller)?.querySelector<HTMLElement>(`[data-ctx="${op}"]`);
  button?.click();
  return !!button;
}
