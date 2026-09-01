import { beforeEach, describe, expect, test } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useReviewed } from "./useReviewed";
import type { PrFile } from "../core/github";
import type { PrRef } from "../core/lib";

const ref: PrRef = { owner: "o", repo: "r", number: 7 };
const KEY = "deltoids.review.viewed:o/r/7";

function file(filename: string, sha: string): PrFile {
  return { filename, status: "modified", sha };
}

const a = file("src/a.ts", "sha-a");
const b = file("src/b.ts", "sha-b");

describe("useReviewed", () => {
  beforeEach(() => localStorage.clear());

  test("a file is reviewed only when its stored sha matches the current one", () => {
    localStorage.setItem(KEY, JSON.stringify({ "src/a.ts": "sha-a" }));
    const { result } = renderHook(() => useReviewed(ref, [a, b]));
    expect(result.current.isReviewed(a)).toBe(true);
    expect(result.current.isReviewed(b)).toBe(false);
    expect(result.current.count).toBe(1);
  });

  test("a changed file (new blob sha) auto-unmarks", () => {
    localStorage.setItem(KEY, JSON.stringify({ "src/a.ts": "sha-a" }));
    const changed = file("src/a.ts", "sha-a2");
    const { result } = renderHook(() => useReviewed(ref, [changed]));
    expect(result.current.isReviewed(changed)).toBe(false);
    expect(result.current.count).toBe(0);
  });

  test("toggle marks, persists, and unmarks", () => {
    const { result } = renderHook(() => useReviewed(ref, [a, b]));
    act(() => result.current.toggle(a));
    expect(result.current.isReviewed(a)).toBe(true);
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual({ "src/a.ts": "sha-a" });

    act(() => result.current.toggle(a));
    expect(result.current.isReviewed(a)).toBe(false);
    // Empty map clears the key entirely.
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  test("clear empties every mark and removes the key", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({ "src/a.ts": "sha-a", "src/b.ts": "sha-b" }),
    );
    const { result } = renderHook(() => useReviewed(ref, [a, b]));
    expect(result.current.count).toBe(2);
    act(() => result.current.clear());
    expect(result.current.count).toBe(0);
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  test("a corrupt stored value is treated as empty", () => {
    localStorage.setItem(KEY, "{not json");
    const { result } = renderHook(() => useReviewed(ref, [a]));
    expect(result.current.isReviewed(a)).toBe(false);
    expect(result.current.count).toBe(0);
  });

  test("a file without a sha is never reviewed and cannot be toggled", () => {
    const noSha: PrFile = { filename: "x.ts", status: "modified" };
    const { result } = renderHook(() => useReviewed(ref, [noSha]));
    act(() => result.current.toggle(noSha));
    expect(result.current.isReviewed(noSha)).toBe(false);
    expect(localStorage.getItem(KEY)).toBeNull();
  });
});
