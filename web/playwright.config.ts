import { defineConfig, devices } from "@playwright/test";

// End-to-end: the built UI served by a real daemon. Point it at one with
//   RECUERDOS_URL=http://127.0.0.1:7070 RECUERDOS_KEY=ra_live_… pnpm e2e
// The key must be a fresh user's, with read,write: the tests seed and change
// that user's memories. Start that daemon with
// RECUERDOS_AI_RATE_LIMIT__ENABLED=false: the suite loads every tab in a burst
// no person makes, and the default budget answers it with 429s.
export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: process.env.RECUERDOS_URL ?? "http://127.0.0.1:7070",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
