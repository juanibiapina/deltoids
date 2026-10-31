import { defineConfig, devices } from "@playwright/test";

// Browser checks that need real layout (jsdom has none). `npm run e2e` builds
// the review core; the engine must already be in public/ (see AGENTS.md).
// Desktop projects run the desktop specs; phone projects run phone.e2e.ts.
const desktop = ["scroll.e2e.ts", "context.e2e.ts"];

export default defineConfig({
  testDir: "test/e2e",
  fullyParallel: true,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: "http://localhost:5174" },
  projects: [
    { name: "chromium", testMatch: desktop, use: { ...devices["Desktop Chrome"] } },
    { name: "webkit", testMatch: desktop, use: { ...devices["Desktop Safari"] } },
    { name: "android", testMatch: "phone.e2e.ts", use: { ...devices["Pixel 7"] } },
    { name: "iphone", testMatch: "phone.e2e.ts", use: { ...devices["iPhone 13"] } },
  ],
  webServer: {
    command: "npx vite --mode e2e --port 5174 --strictPort",
    url: "http://localhost:5174",
    reuseExistingServer: !process.env.CI,
  },
});
