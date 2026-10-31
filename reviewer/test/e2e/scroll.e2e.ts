import { expect, test, type Page } from "@playwright/test";
import { FILES, servePullRequest } from "./fixture";

// Pixel-perfect scrolling: whatever changes size above the reading
// position, the text on screen stays where it is.

const SCROLLER = ".pane-scroll";

async function openAllFiles(page: Page): Promise<void> {
  await servePullRequest(page);
  await page.goto("/?pr=o/r/1");
  await page.getByRole("tree").getByText("src", { exact: true }).click();
  await expect(page.locator(".file:not([hidden])")).toHaveCount(FILES, { timeout: 20_000 });
  await expect(page.locator(".file:not([hidden]) .row").first()).toBeVisible({ timeout: 20_000 });
}

// The element at the top edge of the scroller, tagged so it can be found
// again, and its distance from that edge.
async function markTop(page: Page): Promise<number> {
  return page.evaluate((selector) => {
    const scroller = document.querySelector<HTMLElement>(selector)!;
    const box = scroller.getBoundingClientRect();
    for (const el of document.elementsFromPoint(box.left + box.width / 3, box.top + 1)) {
      const row = el.closest<HTMLElement>(".row, .file-head");
      if (row && scroller.contains(row)) {
        row.dataset.mark = "top";
        return row.getBoundingClientRect().top - box.top;
      }
    }
    throw new Error("no row at the top of the scroller");
  }, SCROLLER);
}

async function markedOffset(page: Page): Promise<number> {
  return page.evaluate((selector) => {
    const box = document.querySelector<HTMLElement>(selector)!.getBoundingClientRect();
    const row = document.querySelector<HTMLElement>("[data-mark=top]");
    if (!row) throw new Error("the marked row is gone");
    return row.getBoundingClientRect().top - box.top;
  }, SCROLLER);
}

async function settle(page: Page): Promise<void> {
  await expect(page.locator(".file:not([hidden]) .skeleton")).toHaveCount(0, { timeout: 15_000 });
  await page.waitForTimeout(300);
}

test("cards loading above the reading position do not move the text", async ({ page }) => {
  await openAllFiles(page);
  await page.locator(SCROLLER).evaluate((el) => {
    el.scrollTop = el.scrollHeight * 0.6;
  });
  const cardHead = await page.evaluate((selector) => {
    const scroller = document.querySelector<HTMLElement>(selector)!;
    const box = scroller.getBoundingClientRect();
    const heads = [...scroller.querySelectorAll<HTMLElement>(".file:not([hidden]) .file-head")];
    const head = heads.find((h) => h.getBoundingClientRect().top >= box.top)!;
    head.dataset.mark = "top";
    return head.getBoundingClientRect().top - box.top;
  }, SCROLLER);
  await settle(page);
  expect(Math.abs((await markedOffset(page)) - cardHead)).toBeLessThan(1);
});

test("a theme switch keeps the text still", async ({ page }) => {
  await openAllFiles(page);
  await settleAt(page, 0.4);
  const before = await markTop(page);
  await page.getByLabel("Syntax theme").first().selectOption({ index: 3 });
  await page.waitForTimeout(800);
  const row = await page.evaluate(() => {
    const scroller = document.querySelector<HTMLElement>(".pane-scroll")!;
    const box = scroller.getBoundingClientRect();
    for (const el of document.elementsFromPoint(box.left + box.width / 3, box.top + 1)) {
      const r = el.closest<HTMLElement>(".row, .file-head");
      if (r) return r.getBoundingClientRect().top - box.top;
    }
    return NaN;
  });
  expect(Math.abs(row - before)).toBeLessThan(1);
});

test("revealing a gap above the reading position does not move the text", async ({ page }) => {
  await openAllFiles(page);
  await settleAt(page, 0.5);
  const before = await markTop(page);
  const revealed = await page.evaluate((selector) => {
    const box = document.querySelector<HTMLElement>(selector)!.getBoundingClientRect();
    const gaps = [...document.querySelectorAll<HTMLElement>(".file:not([hidden]) .gap")];
    const above = gaps.filter((g) => g.getBoundingClientRect().bottom < box.top).pop();
    above?.click();
    return !!above;
  }, SCROLLER);
  expect(revealed).toBe(true);
  await page.waitForTimeout(800);
  expect(Math.abs((await markedOffset(page)) - before)).toBeLessThan(1);
});

async function settleAt(page: Page, fraction: number): Promise<void> {
  for (let i = 0; i < 3; i++) {
    await page.locator(SCROLLER).evaluate((el, f) => {
      el.scrollTop = el.scrollHeight * f;
    }, fraction);
    await settle(page);
  }
}
