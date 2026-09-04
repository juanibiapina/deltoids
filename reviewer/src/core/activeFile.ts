// Pure policy for the file-tree scrollspy: given the set of file-card indices
// currently intersecting the top detection band, decide which one is "active"
// (highlighted in the tree). File cards render in index order, so the topmost
// visible card is the smallest intersecting index. When nothing intersects —
// the small gaps between cards, or the very top/bottom of the page — hold the
// previous value so the highlight does not flicker off.
//
// Kept DOM-free so it can be unit-tested without an IntersectionObserver
// (jsdom has none), mirroring `decideShown` in useChromeCollapse.
export function pickActiveIndex(
  intersecting: Set<number>,
  prev: number | null,
): number | null {
  if (intersecting.size === 0) return prev;
  return Math.min(...intersecting);
}
