import { describe, expect, test } from "vitest";
import { decideShown, type CollapseInput } from "./useChromeCollapse";

const base: CollapseInput = {
  prevY: 200,
  curY: 200,
  threshold: 6,
  atTop: false,
  focusWithin: false,
  menuOpen: false,
  pinning: false,
  current: true,
};

describe("decideShown", () => {
  test("hides when scrolled down past the threshold", () => {
    expect(decideShown({ ...base, prevY: 200, curY: 220 })).toBe(false);
  });

  test("reveals when scrolled up past the threshold", () => {
    expect(decideShown({ ...base, current: false, prevY: 220, curY: 200 })).toBe(true);
  });

  test("holds the current state within the threshold", () => {
    expect(decideShown({ ...base, current: false, prevY: 200, curY: 203 })).toBe(false);
    expect(decideShown({ ...base, current: true, prevY: 200, curY: 197 })).toBe(true);
  });

  test("always shows at the top", () => {
    expect(decideShown({ ...base, current: false, atTop: true, prevY: 50, curY: 80 })).toBe(true);
  });

  test("always shows while the header holds focus", () => {
    expect(decideShown({ ...base, current: false, focusWithin: true, prevY: 200, curY: 260 })).toBe(true);
  });

  test("always shows while the settings popover is open", () => {
    expect(decideShown({ ...base, current: false, menuOpen: true, prevY: 200, curY: 260 })).toBe(true);
  });

  test("always shows while a file jump is pinned", () => {
    expect(decideShown({ ...base, current: false, pinning: true, prevY: 200, curY: 260 })).toBe(true);
  });
});
