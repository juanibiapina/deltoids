export type ChangeKind = "added" | "removed";

export interface Span {
  top: number;
  height: number;
  kind: ChangeKind;
}

export interface Cell {
  added: boolean;
  removed: boolean;
}

export interface Geometry {
  contentHeight: number;
  track: number;
}

export const CELL_PX = 3;
export const MIN_THUMB_PX = 24;

export function changeCells(spans: Span[], g: Geometry, cellPx = CELL_PX): Cell[] {
  const count = Math.max(0, Math.floor(g.track / cellPx));
  const cells = Array.from({ length: count }, () => ({ added: false, removed: false }));
  if (count === 0 || g.contentHeight <= 0) return cells;

  const fits = g.contentHeight <= g.track;
  const cellAt = (y: number): number => {
    const clamped = Math.min(Math.max(y, 0), g.contentHeight - 1);
    const cell = fits
      ? Math.floor(clamped / cellPx)
      : g.contentHeight <= 1
        ? 0
        : Math.floor((clamped * (count - 1)) / (g.contentHeight - 1));
    return Math.min(cell, count - 1);
  };

  for (const span of spans) {
    if (span.height <= 0) continue;
    const first = cellAt(span.top);
    const last = cellAt(span.top + span.height - 1);
    for (let i = first; i <= last; i++) cells[i][span.kind] = true;
  }
  return cells;
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
