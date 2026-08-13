// The Phase-2 exit gate, exercised end-to-end and keyboard-only: write →
// save → search → open, with the shell touching the vault exclusively
// through composd. Then the Phase-3 loop over the same boundary: an agent
// proposes, the reviewer accepts hunk-by-hunk, staleness is flagged and
// enforced.

import { expect, test } from "@playwright/test";
import { agentPropose, gotoConnected } from "./helpers";

test("keyboard-only: write, save, search, open", async ({ page }) => {
  await gotoConnected(page);

  // Palette → New document.
  await page.keyboard.press("ControlOrMeta+p");
  await expect(page.getByRole("combobox")).toBeFocused();
  await page.keyboard.type("new doc");
  await page.keyboard.press("Enter");

  // Path prompt.
  await expect(page.getByLabel(/Vault path/)).toBeFocused();
  await page.keyboard.type("notes/loop.md");
  await page.keyboard.press("Enter");

  // The editor takes focus; write and save.
  await expect(page.locator(".cm-content")).toBeFocused();
  await page.keyboard.type("# Loop Test\n\nsearchable zanzibar body");
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("status")).toContainText("Saved notes/loop.md");

  // Search finds it; open the hit.
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByLabel(/Search the vault/)).toBeFocused();
  await page.keyboard.type("zanzibar");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "1 result" })).toBeVisible();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: /notes\/loop\.md/ }),
  ).toBeFocused();
  await page.keyboard.press("Enter");

  // Back in Write with the document open, editor focused.
  await expect(page.locator(".cm-content")).toBeFocused();
  await expect(page.locator(".cm-content")).toContainText("zanzibar");
  await expect(page.getByRole("navigation", { name: "Documents" })).toContainText(
    "notes/loop.md",
  );
});

test("agent proposes, reviewer accepts one of two hunks", async ({ page }) => {
  await gotoConnected(page);

  // Seed the document through the shell.
  await page.keyboard.press("ControlOrMeta+p");
  await expect(page.getByRole("combobox")).toBeFocused();
  await page.keyboard.type("new doc");
  await page.keyboard.press("Enter");
  await expect(page.getByLabel(/Vault path/)).toBeFocused();
  await page.keyboard.type("notes/review.md");
  await page.keyboard.press("Enter");
  await expect(page.locator(".cm-content")).toBeFocused();
  await page.keyboard.type("alpha\nbeta\ngamma");
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("status")).toContainText("Saved notes/review.md");

  // The agent proposes over the RPC boundary (propose-capped role).
  await agentPropose("notes/review.md", [
    { start: 0, del: 1, ins: "ALPHA\n" },
    { start: 2, del: 1, ins: "GAMMA\n" },
  ]);

  // Review mode auto-selects the newest proposal — the one just created.
  await page.keyboard.press("Alt+Digit4");
  await expect(
    page.getByRole("heading", { name: "notes/review.md" }),
  ).toBeVisible();
  await expect(page.getByRole("group", { name: /Hunk 1/ })).toBeVisible();
  await expect(page.getByRole("group", { name: /Hunk 2/ })).toBeVisible();

  // Keep hunk 1, drop hunk 2, accept.
  const second = page.getByRole("checkbox", { name: /Hunk 2/ });
  await second.focus();
  await page.keyboard.press("Space");
  await expect(second).not.toBeChecked();
  const accept = page.getByRole("button", { name: "Accept 1 of 2" });
  await accept.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toContainText(
    "Accepted 1 hunk into notes/review.md",
  );

  // The buffer refreshed from composd: hunk 1 landed, hunk 2 did not.
  await page.keyboard.press("Alt+Digit1");
  await expect(page.locator(".cm-content")).toContainText("ALPHA");
  await expect(page.locator(".cm-content")).toContainText("gamma");
});

test("a stale proposal is flagged and cannot be accepted", async ({ page }) => {
  await gotoConnected(page);

  await page.keyboard.press("ControlOrMeta+p");
  await expect(page.getByRole("combobox")).toBeFocused();
  await page.keyboard.type("new doc");
  await page.keyboard.press("Enter");
  await expect(page.getByLabel(/Vault path/)).toBeFocused();
  await page.keyboard.type("notes/stale.md");
  await page.keyboard.press("Enter");
  await expect(page.locator(".cm-content")).toBeFocused();
  await page.keyboard.type("one\ntwo");
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("status")).toContainText("Saved notes/stale.md");

  await agentPropose("notes/stale.md", [{ start: 0, del: 1, ins: "ONE\n" }]);

  // The user keeps writing; the save strands the proposal.
  await page.locator(".cm-content").click();
  await page.keyboard.press("End");
  await page.keyboard.type(" more");
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("status")).toContainText("Saved notes/stale.md");

  await page.keyboard.press("Alt+Digit4");
  await expect(page.getByRole("heading", { name: "notes/stale.md" })).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Stale");
  await expect(page.getByRole("button", { name: /Accept 1 of 1/ })).toBeDisabled();
});
