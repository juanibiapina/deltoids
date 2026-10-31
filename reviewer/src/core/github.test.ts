import { afterEach, describe, expect, test, vi } from "vitest";
import {
  fetchPr,
  loadSides,
  rateRemaining,
  resetRateRemaining,
  searchPrs,
} from "./github";

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

  test("a pull request search goes through the Worker and returns the items", async () => {
    const item = { html_url: "https://github.com/o/r/pull/1", number: 1 };
    const fetch = stubFetch(() => Response.json({ total_count: 1, items: [item] }));
    expect(await searchPrs("author:@me")).toEqual([item]);
    const url = new URL(fetch.mock.calls[0][0], "https://review.deltoids.dev");
    expect(url.pathname).toBe("/api/github/search/issues");
    expect(url.searchParams.get("q")).toBe("is:pr is:open archived:false author:@me");
    expect(url.searchParams.get("sort")).toBe("updated");
    expect(url.searchParams.get("order")).toBe("desc");
    expect(url.searchParams.get("per_page")).toBe("30");
  });

  test("resetRateRemaining forgets the last seen budget", async () => {
    stubFetch(() => Response.json(pr, { headers: { "x-ratelimit-remaining": "3" } }));
    await fetchPr(ref, false);
    expect(rateRemaining()).toBe(3);
    resetRateRemaining();
    expect(rateRemaining()).toBeNull();
  });
});
