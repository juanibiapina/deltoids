import type { Page } from "@playwright/test";

// A recorded-style pull request served in place of api.github.com: FILES
// modified TypeScript files under src/, each with three separated changes,
// so every card has several hunks and gap dividers.

export const FILES = 14;
const LINES = 320;
const BASE = "b".repeat(40);
const HEAD = "h".repeat(40);

function before(file: number): string {
  return Array.from({ length: LINES }, (_, i) => `export const v${file}_${i} = ${i};`).join("\n") + "\n";
}

function after(file: number): string {
  const lines = before(file).trimEnd().split("\n");
  for (const at of [260, 150, 40]) {
    lines.splice(at, 1, `export const v${file}_${at} = ${at} + 1;`, `export const extra${file}_${at} = 0;`);
  }
  return lines.join("\n") + "\n";
}

export async function servePullRequest(page: Page): Promise<void> {
  await page.route("https://api.github.com/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown) =>
      route.fulfill({ json: body, headers: { "x-ratelimit-remaining": "59" } });
    if (url.pathname === "/repos/o/r/pulls/1") {
      return json({ number: 1, title: "Fixture", additions: 0, deletions: 0, base: { sha: BASE }, head: { sha: HEAD } });
    }
    if (url.pathname === "/repos/o/r/pulls/1/files") {
      return json(
        Array.from({ length: FILES }, (_, i) => ({
          filename: `src/file${String(i).padStart(2, "0")}.ts`,
          status: "modified",
          additions: 6,
          deletions: 3,
        })),
      );
    }
    const content = url.pathname.match(/^\/repos\/o\/r\/contents\/src\/file(\d+)\.ts$/);
    if (content) {
      const file = Number(content[1]);
      const ref = url.searchParams.get("ref");
      return route.fulfill({ body: ref === BASE ? before(file) : after(file) });
    }
    return route.fulfill({ status: 404, body: "not in fixture" });
  });
}
