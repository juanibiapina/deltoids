// The desktop z/x keys act on the hunk at the top of the scroller. The key
// handler finds the hunk; its FileCard listens for RESHAPE_EVENT and asks
// the engine to grow or shrink it.

export const RESHAPE_EVENT = "deltoids:reshape";

export interface ReshapeDetail {
  op: "expand" | "shrink";
  hunk: number;
}

// The hunk under the top edge of the scroller, else the first one below it.
export function hunkAtTop(scroller: HTMLElement): HTMLElement | null {
  const top = scroller.getBoundingClientRect().top + 1;
  const hunks = scroller.querySelectorAll<HTMLElement>(".file:not([hidden]) .hunk[data-hunk]");
  return [...hunks].find((h) => h.getBoundingClientRect().bottom > top) ?? null;
}

// Grow or shrink the context of the hunk at the top of the scroller.
export function reshapeHunkAtTop(scroller: HTMLElement, op: "expand" | "shrink"): boolean {
  const hunk = hunkAtTop(scroller);
  if (!hunk) return false;
  const detail: ReshapeDetail = { op, hunk: Number(hunk.dataset.hunk) };
  hunk.dispatchEvent(new CustomEvent(RESHAPE_EVENT, { detail, bubbles: true }));
  return true;
}
