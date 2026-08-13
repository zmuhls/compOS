// E2E + a11y harness (ARCHITECTURE.md §16 Phase 2 gates): the suite runs
// the real stack — a debug-build composd over a scratch vault, the built
// shell served by vite preview, and Chromium driving both. Nothing is
// mocked; the shell touches the vault only through composd, and the tests
// prove it.

import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  workers: 1, // one composd, one vault, one deterministic order
  globalSetup: "./tests/global-setup",
  use: {
    baseURL: "http://127.0.0.1:4173",
  },
  webServer: {
    command: "pnpm build && pnpm preview --host 127.0.0.1 --port 4173 --strictPort",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 120_000,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
