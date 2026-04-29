// @ts-check
import { defineConfig } from "astro/config";

// https://astro.build/config
export default defineConfig({
  site: "https://deltoids.dev",
  // Custom apex domain → no `base` needed.
  build: {
    inlineStylesheets: "auto",
  },
  prefetch: false,
});
