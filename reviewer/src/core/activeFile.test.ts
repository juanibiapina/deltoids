import { describe, expect, test } from "vitest";
import { pickActiveIndex } from "./activeFile";

describe("pickActiveIndex", () => {
  test("picks the topmost (smallest) intersecting index", () => {
    expect(pickActiveIndex(new Set([3, 1, 2]), null)).toBe(1);
  });

  test("holds the previous value when nothing intersects", () => {
    expect(pickActiveIndex(new Set(), 4)).toBe(4);
  });

  test("returns null when nothing intersects and there is no previous", () => {
    expect(pickActiveIndex(new Set(), null)).toBe(null);
  });

  test("a single intersecting index is active", () => {
    expect(pickActiveIndex(new Set([7]), 2)).toBe(7);
  });
});
