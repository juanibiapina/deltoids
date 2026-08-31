import { expect, test } from "vitest";
import { estimateCardHeight } from "./cardHeight";

test("reserves a floor for tiny or unknown diffs", () => {
  expect(estimateCardHeight(0, 0)).toBe(46 + 3 * 20);
  expect(estimateCardHeight(undefined, undefined)).toBe(46 + 3 * 20);
});

test("scales with changed lines", () => {
  expect(estimateCardHeight(10, 5)).toBe(46 + 15 * 20);
});

test("caps huge files", () => {
  expect(estimateCardHeight(9000, 9000)).toBe(46 + 1200 * 20);
});
