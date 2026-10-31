import { expect, test, type Page } from "@playwright/test";
import { servePullRequest } from "./fixture";

// The phone layout, opened with ?layout=phone so it does not depend on the
// stored preference. A review opens on the file list.

const tree = (page: Page) => page.getByRole("tree", { name: "Changed files" });
const shownPath = (page: Page) => page.locator(".file:not([hidden]) .path-text");

test.beforeEach(async ({ page }) => {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1&layout=phone");
  await expect(tree(page)).toBeInViewport({ timeout: 20_000 });
});

async function swipe(page: Page, dx: number): Promise<void> {
  await page.locator(".pane").evaluate((pane, dx) => {
    const box = pane.getBoundingClientRect();
    const x = box.left + box.width / 2;
    const y = box.top + box.height / 2;
    // WebKit has no Touch constructor, so plain events carry the touch lists
    // React reads.
    const send = (type: string, touches: object[], changed: object[]) => {
      const event = new Event(type, { bubbles: true });
      Object.defineProperty(event, "touches", { value: touches });
      Object.defineProperty(event, "changedTouches", { value: changed });
      pane.dispatchEvent(event);
    };
    const at = (clientX: number) => ({ identifier: 1, target: pane, clientX, clientY: y });
    send("touchstart", [at(x)], [at(x)]);
    send("touchend", [], [at(x + dx)]);
  }, dx);
}

test("the file list fills the screen and a pick shows that file", async ({ page }) => {
  const sheet = page.locator(".sidebar");
  await expect.poll(async () => (await sheet.boundingBox())!.y).toBeLessThanOrEqual(0.5);
  expect((await sheet.boundingBox())!.height).toBeGreaterThanOrEqual(page.viewportSize()!.height - 1);
  await tree(page).getByText("file03.ts").click();
  await expect(tree(page)).not.toBeInViewport();
  await expect(shownPath(page)).toHaveText("src/file03.ts");
  await expect(page.locator(".diff-scrollbar")).toHaveCount(0);
});

test("the sticky path reopens the file list", async ({ page }) => {
  await tree(page).getByText("file00.ts").click();
  await expect(tree(page)).not.toBeInViewport();
  await page.getByRole("button", { name: "src/file00.ts, show all files" }).click();
  await expect(tree(page)).toBeInViewport();
});

test("a sideways swipe moves between files", async ({ page }) => {
  await tree(page).getByText("file00.ts").click();
  await expect(page.locator(".file:not([hidden]) .row").first()).toBeVisible({ timeout: 20_000 });
  await swipe(page, -150);
  await expect(shownPath(page)).toHaveText("src/file01.ts");
  await swipe(page, 150);
  await expect(shownPath(page)).toHaveText("src/file00.ts");
});

test("tapping a hunk's scope header grows it", async ({ page }) => {
  await tree(page).getByText("file00.ts").click();
  const rows = page.locator(".file:not([hidden]) .row");
  await expect(rows.first()).toBeVisible({ timeout: 20_000 });
  const before = await rows.count();
  await page.locator(".file:not([hidden]) .hunk > .lineno, .file:not([hidden]) .hunk > .breadcrumb").first().tap();
  await expect.poll(() => rows.count()).toBeGreaterThan(before);
});
