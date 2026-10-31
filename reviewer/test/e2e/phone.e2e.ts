import { expect, test, type Page } from "@playwright/test";
import { servePullRequest } from "./fixture";

// The phone layout, opened with ?layout=phone so it does not depend on the
// stored preference.

async function open(page: Page): Promise<void> {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1&layout=phone");
  await expect(page.locator(".file:not([hidden]) .row").first()).toBeVisible({ timeout: 20_000 });
}

const path = (page: Page) => page.locator(".file:not([hidden]) .file-head .path");

test("shows one file with a bottom bar and no sidebar column", async ({ page }) => {
  await open(page);
  const bar = page.getByRole("navigation", { name: "Review" });
  await expect(bar).toBeVisible();
  for (const button of await bar.getByRole("button").all()) {
    expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  }
  await expect(page.locator(".diff-scrollbar")).toHaveCount(0);
  const tree = page.getByRole("tree", { name: "Changed files" });
  await expect(tree).not.toBeInViewport();
});

test("the file list opens as a sheet and picks a file", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Files" }).click();
  const tree = page.getByRole("tree", { name: "Changed files" });
  await expect(tree).toBeInViewport();
  await tree.getByText("file03.ts").click();
  await expect(path(page)).toHaveText("src/file03.ts");
  await expect(tree).not.toBeInViewport();
});

test("the header steps to the next and previous file", async ({ page }) => {
  await open(page);
  await expect(path(page)).toHaveText("src/file00.ts");
  await expect(page.getByRole("button", { name: "Previous file" })).toBeDisabled();
  await page.getByRole("button", { name: "Next file" }).click();
  await expect(path(page)).toHaveText("src/file01.ts");
  await page.getByRole("button", { name: "Previous file" }).click();
  await expect(path(page)).toHaveText("src/file00.ts");
});

test("the bar jumps between hunks and grows the one at the top", async ({ page }) => {
  await open(page);
  const scroller = page.locator(".pane-scroll");
  const top = () => scroller.evaluate((el) => el.scrollTop);
  await page.getByRole("button", { name: "Next hunk" }).click();
  const first = await top();
  await page.getByRole("button", { name: "Next hunk" }).click();
  const second = await top();
  expect(second).toBeGreaterThan(first);
  await page.getByRole("button", { name: "Previous hunk" }).click();
  expect(await top()).toBe(first);

  const rows = page.locator(".file:not([hidden]) .row");
  const before = await rows.count();
  await page.getByRole("button", { name: "More context" }).click();
  await expect.poll(() => rows.count()).toBeGreaterThan(before);
  await page.getByRole("button", { name: "Less context" }).click();
  await expect.poll(() => rows.count()).toBe(before);
});
