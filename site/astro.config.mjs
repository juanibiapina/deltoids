// @ts-check
import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";

// https://astro.build/config
export default defineConfig({
  site: "https://deltoids.dev",
  // Custom apex domain → no `base` needed.
  integrations: [sitemap()],
  build: {
    // Single ~10KB stylesheet on a one-page site — inline it to drop the
    // render-blocking request (Astro's `auto` only inlines below ~4KB).
    inlineStylesheets: "always",
  },
  prefetch: false,
});
