// Estimate a file card's rendered height from its changed-line count, so the
// skeleton can reserve roughly that space before content loads. Stable layout
// means jumping to a card lands accurately and cards loading above it barely
// shift the page. The estimate is deliberately rough: the goal is to shrink the
// shift, not to be exact.

const ROW_PX = 20; // one diff row at the default font (~13px, line-height 1.5)
const BASE_PX = 46; // sticky file-head + card chrome
const MIN_ROWS = 3;
const MAX_ROWS = 1200; // cap so a huge file does not reserve an absurd column

export function estimateCardHeight(
  additions: number | undefined,
  deletions: number | undefined,
): number {
  const changed = (additions ?? 0) + (deletions ?? 0);
  const rows = Math.min(MAX_ROWS, Math.max(MIN_ROWS, changed));
  return BASE_PX + rows * ROW_PX;
}
