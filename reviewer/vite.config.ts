/// <reference types="vitest/config" />
import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

// Standalone reviewer app. Served at the root of review.deltoids.dev, so the
// base path is "/" and the wasm engine resolves at "/deltoids_wasm.wasm".
export default defineConfig(({ command, mode }) => {
  if (command === "build" && !loadEnv(mode, ".").VITE_CLERK_PUBLISHABLE_KEY) {
    throw new Error("VITE_CLERK_PUBLISHABLE_KEY is required to build the reviewer");
  }
  return {
    base: "/",
    plugins: [react()],
    // Playwright runs `vite --mode e2e` with Clerk replaced by a signed-out stub.
    resolve:
      mode === "e2e"
        ? { alias: { "@clerk/react": new URL("./test/e2e/clerk.tsx", import.meta.url).pathname } }
        : undefined,
    build: {
      target: "es2022",
    },
    worker: {
      format: "es",
    },
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./test/setup.ts"],
    },
  };
});
