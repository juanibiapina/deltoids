import { expect, test } from "@playwright/test";
import { servePullRequest } from "./fixture";

test.beforeEach(async ({ page }) => {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1");
  await expect(page.locator(".file:not([hidden]) .row").first()).toBeVisible({ timeout: 20_000 });
});

test("z and x grow and shrink the hunk at the reading position", async ({ page }) => {
  const rows = page.locator(".file:not([hidden]) .row");
  const before = await rows.count();
  await page.keyboard.press("z");
  await expect.poll(() => rows.count()).toBeGreaterThan(before);
  await page.keyboard.press("x");
  await expect.poll(() => rows.count()).toBe(before);
});

test("clicking a hunk's scope header grows the hunk", async ({ page }) => {
  const rows = page.locator(".file:not([hidden]) .row");
  const before = await rows.count();
  await page.locator(".file:not([hidden]) .hunk > .lineno, .file:not([hidden]) .hunk > .breadcrumb").first().click();
  await expect.poll(() => rows.count()).toBeGreaterThan(before);
});
