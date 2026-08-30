import { test } from "node:test";
import assert from "node:assert/strict";

import {
  parsePrUrl,
  decodeBase64Utf8,
  looksBinary,
  badgeClass,
} from "../public/review/lib.js";

test("parsePrUrl accepts a full PR URL", () => {
  assert.deepEqual(parsePrUrl("https://github.com/octocat/Spoon-Knife/pull/41131"), {
    owner: "octocat",
    repo: "Spoon-Knife",
    number: 41131,
  });
});

test("parsePrUrl accepts owner/repo/number shorthand", () => {
  assert.deepEqual(parsePrUrl("octocat/Spoon-Knife/41131"), {
    owner: "octocat",
    repo: "Spoon-Knife",
    number: 41131,
  });
});

test("parsePrUrl tolerates trailing paths and query", () => {
  assert.deepEqual(parsePrUrl("https://github.com/a/b/pull/7/files?w=1"), {
    owner: "a",
    repo: "b",
    number: 7,
  });
});

test("parsePrUrl returns null for non-PR input", () => {
  assert.equal(parsePrUrl("https://github.com/a/b"), null);
  assert.equal(parsePrUrl(""), null);
  assert.equal(parsePrUrl("not a url"), null);
});

test("decodeBase64Utf8 decodes UTF-8 including newlines in the base64", () => {
  const b64 = Buffer.from("héllo\nworld", "utf8").toString("base64");
  const chunked = b64.slice(0, 4) + "\n" + b64.slice(4);
  assert.equal(decodeBase64Utf8(chunked), "héllo\nworld");
});

test("looksBinary detects a NUL byte", () => {
  assert.equal(looksBinary("plain text"), false);
  assert.equal(looksBinary("has\u0000nul"), true);
});

test("badgeClass maps statuses", () => {
  assert.equal(badgeClass("added"), "added");
  assert.equal(badgeClass("removed"), "removed");
  assert.equal(badgeClass("renamed"), "renamed");
  assert.equal(badgeClass("modified"), "");
});
