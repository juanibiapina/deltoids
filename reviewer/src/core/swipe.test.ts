import { describe, expect, test } from "vitest";
import { createSwipeTracker } from "./swipe";

const free = { atLeft: true, atRight: true };

function swipe(dx: number, dy: number, edges = free): number {
  const t = createSwipeTracker();
  t.start(200, 300, edges);
  return t.end(200 + dx, 300 + dy);
}

describe("createSwipeTracker", () => {
  test("a left swipe steps forward, a right swipe steps back", () => {
    expect(swipe(-80, 5)).toBe(1);
    expect(swipe(80, -5)).toBe(-1);
  });

  test("a short or mostly vertical move does not step", () => {
    expect(swipe(-40, 0)).toBe(0);
    expect(swipe(-80, 60)).toBe(0);
    expect(swipe(80, 200)).toBe(0);
  });

  test("a swipe steps only from the edge it pans toward", () => {
    expect(swipe(-80, 0, { atLeft: true, atRight: false })).toBe(0);
    expect(swipe(80, 0, { atLeft: false, atRight: true })).toBe(0);
    expect(swipe(80, 0, { atLeft: true, atRight: false })).toBe(-1);
    expect(swipe(-80, 0, { atLeft: false, atRight: true })).toBe(1);
  });

  test("a cancelled gesture does not step", () => {
    const t = createSwipeTracker();
    t.start(200, 300, free);
    t.cancel();
    expect(t.end(100, 300)).toBe(0);
  });

  test("an end without a start does not step, and ends clear the start", () => {
    const t = createSwipeTracker();
    expect(t.end(100, 300)).toBe(0);
    t.start(200, 300, free);
    expect(t.end(100, 300)).toBe(1);
    expect(t.end(0, 300)).toBe(0);
  });
});
