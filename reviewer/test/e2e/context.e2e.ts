import { expect, test } from "@playwright/test";
import { servePullRequest } from "./fixture";

test("z and x grow and shrink the hunk at the reading position", async ({ page }) => {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1");
  const rows = page.locator(".file:not([hidden]) .row");
  await expect(rows.first()).toBeVisible({ timeout: 20_000 });
  const before = await rows.count();

  await page.keyboard.press("z");
  await expect.poll(() => rows.count()).toBeGreaterThan(before);

  await page.keyboard.press("x");
  await expect.poll(() => rows.count()).toBe(before);
});

test("a hunk's + button grows its context", async ({ page }) => {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1");
  const rows = page.locator(".file:not([hidden]) .row");
  await expect(rows.first()).toBeVisible({ timeout: 20_000 });
  const before = await rows.count();
  await page.locator(".file:not([hidden]) .hunk").first().getByRole("button", { name: "More context" }).click();
  await expect.poll(() => rows.count()).toBeGreaterThan(before);
});
