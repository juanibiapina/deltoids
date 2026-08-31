/// <reference types="vitest/config" />
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Standalone reviewer app. Served at the root of review.deltoids.dev, so the
// base path is "/" and the wasm engine resolves at "/deltoids_wasm.wasm".
export default defineConfig({
  base: "/",
  plugins: [react()],
  build: {
    target: "es2022",
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});
