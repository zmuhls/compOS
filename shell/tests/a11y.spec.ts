// The axe-core WCAG 2.2 suite (ARCHITECTURE.md §12/§16: CI-gated from
// Phase 2; pattern inherited from comprosody-reader, selectors rewritten
// for this shell). Every mode is swept in a realistic state — with a
// document open, results showing, a proposal under review — plus the
// non-axe obligations: 320 px reflow, reduced motion, forced colors, and
// focus restoration.

import { expect, test } from "@playwright/test";
import { agentPropose, expectAxeClean, gotoConnected } from "./helpers";

async function createDoc(
  page: import("@playwright/test").Page,
  path: string,
  body: string,
): Promise<void> {
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page.getByLabel(/Vault path/).fill(path);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.locator(".cm-content")).toBeFocused();
  await page.keyboard.type(body);
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("status")).toContainText(`Saved ${path}`);
}

test("connect screen is axe-clean", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "CompOS" })).toBeVisible();
  await expectAxeClean(page);
});

test("write mode with an open document is axe-clean", async ({ page }) => {
  await gotoConnected(page);
  await createDoc(page, "a11y/write.md", "# Heading\n\nSome prose to edit.");
  await expectAxeClean(page);
});

test("read mode with rendered markdown is axe-clean", async ({ page }) => {
  await gotoConnected(page);
  await createDoc(
    page,
    "a11y/read.md",
    "# Title\n\n## Section\n\nBody text with a [link](https://example.org).",
  );
  await page.keyboard.press("Alt+Digit2");
  await expect(page.getByRole("navigation", { name: "Contents" })).toBeVisible();
  await expectAxeClean(page);
});

test("search mode with results is axe-clean", async ({ page }) => {
  await gotoConnected(page);
  await createDoc(page, "a11y/search.md", "unique axesearch token body");
  await page.keyboard.press("ControlOrMeta+k");
  const input = page.getByLabel(/Search the vault/);
  await expect(input).toBeFocused();
  await page.keyboard.type("axesearch");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: /result/ })).toBeVisible();
  await expectAxeClean(page);
});

test("review mode with a proposal under review is axe-clean", async ({ page }) => {
  await gotoConnected(page);
  await createDoc(page, "a11y/review.md", "alpha\nbeta\ngamma");
  await agentPropose("a11y/review.md", [
    { start: 0, del: 1, ins: "ALPHA\n" },
    { start: 2, del: 0, ins: "delta\n" },
  ]);
  await page.keyboard.press("Alt+Digit4");
  await expect(page.getByRole("group", { name: /Hunk 1/ })).toBeVisible();
  await expectAxeClean(page);
});

test("command palette is axe-clean and traps focus", async ({ page }) => {
  await gotoConnected(page);
  await page.keyboard.press("ControlOrMeta+p");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expectAxeClean(page);
  // Focus stays inside the dialog when tabbing through it.
  for (let i = 0; i < 8; i++) {
    await page.keyboard.press("Tab");
    const inside = await page.evaluate(() => {
      const dialog = document.querySelector('[role="dialog"]');
      return dialog !== null && dialog.contains(document.activeElement);
    });
    expect(inside).toBe(true);
  }
});

test("closing a dialog restores focus to the opener", async ({ page }) => {
  await gotoConnected(page);
  await createDoc(page, "a11y/focus.md", "focus body");
  await page.locator(".cm-content").click();
  await expect(page.locator(".cm-content")).toBeFocused();
  await page.keyboard.press("ControlOrMeta+p");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".cm-content")).toBeFocused();
});

test("reflow: no horizontal scrolling at 320 px (WCAG 1.4.10)", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 640 });
  await gotoConnected(page);
  for (const key of ["Alt+Digit1", "Alt+Digit3", "Alt+Digit4"]) {
    await page.keyboard.press(key);
    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    );
    expect(overflow, `mode via ${key} must not overflow`).toBeLessThanOrEqual(0);
  }
});

test("reduced motion is honored (WCAG 2.3.3)", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await gotoConnected(page);
  const behavior = await page.evaluate(
    () => getComputedStyle(document.documentElement).scrollBehavior,
  );
  expect(behavior).toBe("auto");
});

test("forced-colors mode stays usable and axe-clean", async ({ page }) => {
  await page.emulateMedia({ forcedColors: "active" });
  await gotoConnected(page);
  await createDoc(page, "a11y/forced.md", "forced colors body");
  await expectAxeClean(page);
});
