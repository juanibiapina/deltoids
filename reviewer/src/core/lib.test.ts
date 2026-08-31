import { describe, expect, test } from "vitest";
import { parsePrUrl, decodeBase64Utf8, looksBinary, badgeClass } from "./lib";

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

test("decodeBase64Utf8 decodes UTF-8 including newlines in the base64", () => {
  const b64 = btoa(
    Array.from(new TextEncoder().encode("héllo\nworld"))
      .map((b) => String.fromCharCode(b))
      .join(""),
  );
  const chunked = b64.slice(0, 4) + "\n" + b64.slice(4);
  expect(decodeBase64Utf8(chunked)).toBe("héllo\nworld");
});

test("looksBinary detects a NUL byte", () => {
  expect(looksBinary("plain text")).toBe(false);
  expect(looksBinary("has\u0000nul")).toBe(true);
});

test("badgeClass maps statuses", () => {
  expect(badgeClass("added")).toBe("added");
  expect(badgeClass("removed")).toBe("removed");
  expect(badgeClass("renamed")).toBe("renamed");
  expect(badgeClass("modified")).toBe("");
});
