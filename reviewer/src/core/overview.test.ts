import { describe, expect, test } from "vitest";
import { changeCells, scrollForThumbTop, thumb, MIN_THUMB_PX, type Span } from "./overview";

const kinds = (cells: { added: boolean; removed: boolean }[]) =>
  cells.map((c) => (c.added && c.removed ? "b" : c.added ? "+" : c.removed ? "-" : "."));

describe("changeCells", () => {
  test("tall content keeps the first, middle, and last changes on the track", () => {
    const spans: Span[] = [
      { top: 0, height: 20, kind: "removed" },
      { top: 5000, height: 20, kind: "added" },
      { top: 9980, height: 20, kind: "removed" },
    ];
    const cells = kinds(changeCells(spans, { contentHeight: 10000, track: 30 }, 3));
    expect(cells).toHaveLength(10);
    expect(cells[0]).toBe("-");
    expect(cells[4]).toBe("+");
    expect(cells[9]).toBe("-");
    expect([1, 2, 3, 5, 6, 7].map((i) => cells[i])).toEqual([".", ".", ".", ".", ".", "."]);
  });

  test("an added and a removed row sharing a cell keep both", () => {
    const spans: Span[] = [
      { top: 0, height: 10, kind: "added" },
      { top: 10, height: 10, kind: "removed" },
    ];
    const cells = changeCells(spans, { contentHeight: 10000, track: 30 }, 3);
    expect(cells[0]).toEqual({ added: true, removed: true });
  });

  test("fitting content lines marks up with the rows", () => {
    const spans: Span[] = [
      { top: 0, height: 3, kind: "added" },
      { top: 9, height: 6, kind: "removed" },
    ];
    const cells = kinds(changeCells(spans, { contentHeight: 20, track: 30 }, 3));
    expect(cells.join("")).toBe("+..--.....");
  });

  test("empty or degenerate geometry does not throw", () => {
    const spans: Span[] = [{ top: 0, height: 5, kind: "added" }];
    expect(changeCells(spans, { contentHeight: 0, track: 30 })).toHaveLength(10);
    expect(changeCells(spans, { contentHeight: 100, track: 0 })).toEqual([]);
    expect(changeCells(spans, { contentHeight: 1, track: 2 }, 3)).toEqual([]);
  });
});

describe("thumb", () => {
  test("is hidden when the content fits", () => {
    expect(thumb(0, { contentHeight: 300, track: 300 })).toBeNull();
  });

  test("spans the viewport share and reaches both ends", () => {
    const g = { contentHeight: 1000, track: 250 };
    expect(thumb(0, g)).toEqual({ top: 0, height: 62.5 });
    expect(thumb(750, g)).toEqual({ top: 187.5, height: 62.5 });
  });

  test("keeps a minimum size on huge content", () => {
    expect(thumb(0, { contentHeight: 1_000_000, track: 500 })?.height).toBe(MIN_THUMB_PX);
  });
});

describe("scrollForThumbTop", () => {
  test("inverts the thumb position and clamps", () => {
    const g = { contentHeight: 1000, track: 250 };
    expect(scrollForThumbTop(187.5, g)).toBe(750);
    expect(scrollForThumbTop(-50, g)).toBe(0);
    expect(scrollForThumbTop(9999, g)).toBe(750);
  });
});
