// @ts-check
import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import mdx from "@astrojs/mdx";

// https://astro.build/config
export default defineConfig({
  site: "https://deltoids.dev",
  // Custom apex domain → no `base` needed.
  integrations: [sitemap(), mdx()],
  markdown: {
    // Keep code blocks monochrome — matches the terminal aesthetic and
    // avoids dragging Shiki's CSS surface into a tiny doc set.
    syntaxHighlight: false,
  },
  build: {
    // Single ~10KB stylesheet on a small site — inline it to drop the
    // render-blocking request (Astro's `auto` only inlines below ~4KB).
    inlineStylesheets: "always",
  },
  prefetch: false,
});
