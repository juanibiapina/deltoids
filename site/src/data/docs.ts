/**
 * Docs sidebar. Single source of truth — same convention as `site.ts`.
 *
 * Each item's `href` is the URL the entry links to. Active state is
 * decided by exact match on `Astro.url.pathname` (trailing slash
 * normalised in `DocsLayout.astro`).
 *
 * To add a page: drop an `.mdx` file under `src/pages/docs/`
 * (e.g. `src/pages/docs/<slug>/index.mdx`) and append an entry below.
 */

export type DocsNavItem = {
  label: string;
  href: string;
};

export type DocsNavGroup = {
  label: string;
  items: DocsNavItem[];
};

export const DOCS_NAV: readonly DocsNavGroup[] = [
  {
    label: "Getting started",
    items: [{ label: "Install", href: "/docs/" }],
  },
] as const;
