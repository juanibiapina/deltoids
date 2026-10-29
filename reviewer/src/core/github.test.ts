import { afterEach, describe, expect, test, vi } from "vitest";
import {
  fetchPr,
  loadSides,
  rateRemaining,
  renderSides,
  resetRateRemaining,
  type Sides,
} from "./github";
import type { Engine } from "./engine";

describe("renderSides", () => {
  test("full sides call renderFile with the theme", () => {
    const engine = {
      renderFile: vi.fn().mockReturnValue("<full>"),
      renderFromPatch: vi.fn(),
    renderContext: vi.fn(),
    } satisfies Engine;
    const sides: Sides = { kind: "full", before: "a", after: "b", path: "x.rs" };

    const html = renderSides(engine, sides, "TokyoNight");

    expect(html).toBe("<full>");
    expect(engine.renderFile).toHaveBeenCalledWith("a", "b", "x.rs", "TokyoNight");
    expect(engine.renderFromPatch).not.toHaveBeenCalled();
  });

  test("patch sides call renderFromPatch with the theme", () => {
    const engine = {
      renderFile: vi.fn(),
      renderFromPatch: vi.fn().mockReturnValue("<patch>"),
    renderContext: vi.fn(),
    } satisfies Engine;
    const sides: Sides = {
      kind: "patch",
      after: "b",
      patch: "@@ -1 +1 @@",
      path: "x.rs",
    };

    const html = renderSides(engine, sides, "GitHub");

    expect(html).toBe("<patch>");
    expect(engine.renderFromPatch).toHaveBeenCalledWith(
      "b",
      "@@ -1 +1 @@",
      "x.rs",
      "GitHub",
    );
    expect(engine.renderFile).not.toHaveBeenCalled();
  });

  test("re-rendering the same sides with a new theme hits no network", () => {
    const engine = {
      renderFile: vi.fn().mockReturnValue("<html>"),
      renderFromPatch: vi.fn(),
    renderContext: vi.fn(),
    } satisfies Engine;
    const sides: Sides = { kind: "full", before: "a", after: "b", path: "x.rs" };

    renderSides(engine, sides, "TokyoNight");
    renderSides(engine, sides, "GitHub");

    // Two pure renders from one cached Sides; the second passes the new theme.
    expect(engine.renderFile).toHaveBeenNthCalledWith(1, "a", "b", "x.rs", "TokyoNight");
    expect(engine.renderFile).toHaveBeenNthCalledWith(2, "a", "b", "x.rs", "GitHub");
  });
});

describe("GitHub requests", () => {
  const ref = { owner: "o", repo: "r", number: 1 };
  const pr = { number: 1, base: { sha: "b" }, head: { sha: "h" } };

  function stubFetch(res: () => Response) {
    const fetch = vi.fn((_url: string, _init?: RequestInit) => Promise.resolve(res()));
    vi.stubGlobal("fetch", fetch);
    return fetch;
  }

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test("signed in, requests go through the Worker without a token", async () => {
    const fetch = stubFetch(() => Response.json(pr));
    await fetchPr(ref, true);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("/api/github/repos/o/r/pulls/1");
    expect(new Headers(init?.headers).has("authorization")).toBe(false);
  });

  test("signed out, requests go straight to GitHub without a token", async () => {
    const fetch = stubFetch(() => Response.json(pr));
    await fetchPr(ref, false);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("https://api.github.com/repos/o/r/pulls/1");
    expect(new Headers(init?.headers).has("authorization")).toBe(false);
  });

  test("a 401 on file contents reports an expired sign-in", async () => {
    stubFetch(() => new Response("", { status: 401 }));
    const file = { filename: "a.ts", status: "modified", patch: "@@ -1 +1 @@" };
    await expect(loadSides(ref, file, "b", "h", true)).rejects.toThrow(
      "Your sign-in expired. Reload the page or sign in again.",
    );
  });

  test("resetRateRemaining forgets the last seen budget", async () => {
    stubFetch(() => Response.json(pr, { headers: { "x-ratelimit-remaining": "3" } }));
    await fetchPr(ref, false);
    expect(rateRemaining()).toBe(3);
    resetRateRemaining();
    expect(rateRemaining()).toBeNull();
  });
});
