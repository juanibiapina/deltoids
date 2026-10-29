# Site

Bare Astro marketing site for `deltoids.dev`, with no integrations and
minimal client JS. The "Try it live" links point at `SITE.reviewUrl`.

## Local dev

```bash
cd site
npm install
npm run dev        # http://localhost:4321 with hot reload
npm run build      # writes site/dist/
npm run preview    # serves site/dist/
npx astro check
```

## Conventions

- `src/data/site.ts` is the single source of truth for the version, copy,
  install commands, repo URLs, and FAQ. Do not hardcode them in pages or
  components.
- `src/data/docs.ts` is the single source of truth for the docs sidebar. To
  add a docs page, create an `.mdx` file under `src/pages/docs/` and append
  an entry to `DOCS_NAV`.
- Docs pages are MDX. Each page sets
  `layout: "../../layouts/DocsLayout.astro"` in its frontmatter and uses H2
  sections per topic. Code blocks stay plain (`markdown.syntaxHighlight:
  false` in `astro.config.mjs`). Use `<Note>` from
  `src/components/docs/Note.astro` for inline callouts.
- Keep client JS minimal. The `<details>` element opens and closes FAQ
  entries. The only script is the click-to-copy handler for `.cmd-line` in
  `Base.astro`.
- Fonts are self-hosted latin-subset woff2 files in `public/fonts/`,
  preloaded in `Base.astro`. Do not switch to the Google Fonts CDN.
- Headings use JetBrains Mono and body text uses IBM Plex Sans. Add no other
  typefaces.
- Use `var(--text-muted)` (#9aa5ce) for secondary text. Use `--text-faint`
  (#565f89) only on layered backgrounds where contrast still passes, never
  on the page background.
- Inline links in body text must have an underline (the global rule covers
  `p a, .answer a, .faq a`). Lighthouse rejects links that differ by color
  only.

## Deployment

`.github/workflows/pages.yml` builds only `site/**` and deploys to GitHub
Pages on push to `main` when files under `site/**` change. The custom
domain comes from `public/CNAME`.
