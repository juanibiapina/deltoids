import { defineConfig, devices } from "@playwright/test";

// Browser checks that need real layout (jsdom has none). `npm run e2e`
// builds both wasm modules' prerequisites only for the core; the engine must
// already be in public/ (see AGENTS.md).
export default defineConfig({
  testDir: "test/e2e",
  testMatch: "*.e2e.ts",
  fullyParallel: true,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: "http://localhost:5174" },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: {
    command: "npx vite --mode e2e --port 5174 --strictPort",
    url: "http://localhost:5174",
    reuseExistingServer: !process.env.CI,
  },
});
