import { describe, expect, test } from "vitest";
import { parsePrUrl, looksBinary } from "./lib";

describe("parsePrUrl", () => {
  test("accepts a full PR URL", () => {
    expect(parsePrUrl("https://github.com/octocat/Spoon-Knife/pull/41131")).toEqual({
      owner: "octocat",
      repo: "Spoon-Knife",
      number: 41131,
    });
  });

  test("accepts owner/repo/number shorthand", () => {
    expect(parsePrUrl("octocat/Spoon-Knife/41131")).toEqual({
      owner: "octocat",
      repo: "Spoon-Knife",
      number: 41131,
    });
  });

  test("tolerates trailing paths and query", () => {
    expect(parsePrUrl("https://github.com/a/b/pull/7/files?w=1")).toEqual({
      owner: "a",
      repo: "b",
      number: 7,
    });
  });

  test("returns null for non-PR input", () => {
    expect(parsePrUrl("https://github.com/a/b")).toBeNull();
    expect(parsePrUrl("")).toBeNull();
    expect(parsePrUrl("not a url")).toBeNull();
  });
});

test("looksBinary detects a NUL byte", () => {
  expect(looksBinary("plain text")).toBe(false);
  expect(looksBinary("has\u0000nul")).toBe(true);
});
