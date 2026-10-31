import {
  changeCells as coreChangeCells,
  type ChangeKind,
  type CoreCell,
  type CoreSpan,
} from "./reviewCore";

export type { ChangeKind };
export type Span = CoreSpan;
export type Cell = CoreCell;

export interface Geometry {
  contentHeight: number;
  track: number;
}

export const CELL_PX = 3;
export const MIN_THUMB_PX = 24;

// The overview cells come from the shared core
// (`crates/deltoids-review/src/overview.rs`), the same code as the TUI's.
export function changeCells(spans: Span[], g: Geometry, cellPx = CELL_PX): Cell[] {
  return coreChangeCells(spans, g.contentHeight, g.track, cellPx);
}

export function thumb(scrollTop: number, g: Geometry): { top: number; height: number } | null {
  if (g.track <= 0 || g.contentHeight <= g.track) return null;
  const height = Math.min(g.track, Math.max(MIN_THUMB_PX, (g.track * g.track) / g.contentHeight));
  const maxScroll = g.contentHeight - g.track;
  const ratio = Math.min(Math.max(scrollTop / maxScroll, 0), 1);
  return { top: (g.track - height) * ratio, height };
}

export function scrollForThumbTop(thumbTop: number, g: Geometry): number {
  const t = thumb(0, g);
  if (!t) return 0;
  const room = g.track - t.height;
  const maxScroll = g.contentHeight - g.track;
  if (room <= 0) return 0;
  const ratio = Math.min(Math.max(thumbTop / room, 0), 1);
  return ratio * maxScroll;
}
