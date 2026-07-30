/**
 * Docs sidebar. Single source of truth. Same convention as `site.ts`.
 *
 * Each item's `href` is the URL the entry links to. Active state is
 * decided by exact match on `Astro.url.pathname` (trailing slash
 * normalised in `DocsLayout.astro`).
 *
 * To add a page: drop an `.mdx` file under `src/pages/docs/`
 * (e.g. `src/pages/docs/<slug>/index.mdx`) and append an entry below.
 *
 * One entry, one page. Never point an entry at an anchor inside a page
 * that already has its own entry: the reader cannot tell pages from
 * sections, and the exact-match active state highlights the wrong row.
 * A page that needs in-page navigation gets a table of contents, not
 * extra sidebar rows.
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
  {
    label: "Use as a pager",
    items: [
      { label: "Git", href: "/docs/integrations/git/" },
      { label: "GitHub CLI", href: "/docs/integrations/gh/" },
      { label: "Lazygit", href: "/docs/integrations/lazygit/" },
    ],
  },
  {
    label: "Coding agents",
    items: [
      { label: "pi", href: "/docs/integrations/pi/" },
      { label: "Claude Code", href: "/docs/integrations/claude-code/" },
    ],
  },
  {
    label: "Reference",
    items: [{ label: "Configuration", href: "/docs/configuration/" }],
  },
] as const;
