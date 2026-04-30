# Site maintenance

Bare Astro marketing site for `deltoids.dev`. No frameworks beyond Astro
itself. Minimal client JS. Deploys to GitHub Pages.

## Local dev

```bash
cd site
npm install
npm run dev      # http://localhost:4321 with hot reload
npm run build    # writes site/dist/
npm run preview  # serves site/dist/ as it'll be served in prod
```

## Layout

```
site/
├── astro.config.mjs       # site URL = https://deltoids.dev
├── public/                # served verbatim (CNAME, fonts, robots, .nojekyll)
└── src/
    ├── data/site.ts       # version, install commands, FAQ — single source of truth
    ├── styles/global.css  # palette, layout primitives, all section styles
    ├── layouts/Base.astro # head, nav, footer, JSON-LD schema
    ├── components/        # Hero, Features, Pager, Agents, Install, Faq, icons
    └── pages/index.astro  # composes the sections
```

## Conventions

- **`src/data/site.ts` is the single source of truth.** Update there;
  do not hardcode version, install commands, repo URLs, or copy.
- **Keep client JS minimal.** The `<details>` element handles FAQ
  open/close natively. The only script is the inline click-to-copy
  handler in `Base.astro` that targets `.cmd-line` elements. Before
  adding more JS, reconsider whether it's worth the bytes.
- **Fonts are self-hosted** as latin-subset woff2 in `public/fonts/`,
  preloaded in `Base.astro`. Do not switch to Google Fonts CDN.
- **Headings use JetBrains Mono** (terminal aesthetic). Body text
  uses IBM Plex Sans. Avoid mixing in extra typefaces.
- **Color contrast**: use `var(--text-muted)` (#9aa5ce) for secondary
  text. `--text-faint` (#565f89) only on layered backgrounds where
  contrast still passes; never on the page background.
- **Inline links in body text** must have an underline (the global
  rule covers `p a, .answer a, .faq a`). Lighthouse's
  `link-in-text-block` rule rejects color-only differentiation.

## Release checklist

After a new tag in the main project:

1. Bump `SITE.version` in `src/data/site.ts` (drives the hero badge
   and JSON-LD `softwareVersion`).
2. If install commands changed, update `INSTALL_CARDS` in the same
   file.
3. Skim `FAQ` in the same file against the README and CHANGELOG.
4. Build locally, eyeball it, push.

## Deployment

`.github/workflows/pages.yml` builds and deploys on push to `main`
when files under `site/**` change. The custom domain `deltoids.dev`
is wired via `public/CNAME` and the GitHub Pages settings.
