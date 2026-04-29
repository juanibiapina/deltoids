# web — Next.js landing site

Marketing/landing site for `deltoids`, built with Next.js 15 App Router,
Tailwind v4, shadcn-style primitives, Motion, and Vercel OG.

Deployed to Vercel at `deltoids.dev`.

## Stack

- Next.js 15 (App Router, Turbopack)
- Tailwind v4 (no `tailwind.config`; theme tokens in `src/app/globals.css`)
- shadcn/ui primitives, hand-installed under `src/components/ui/`
- Motion for in-view fade/slide reveals
- lucide-react for stroke icons (plus a tiny inline GitHub mark)
- Geist Sans + Geist Mono via `next/font/google`
- `@vercel/og` for static + dynamic social cards
- `@vercel/analytics` for traffic metrics

**No screenshots, Shiki, or Prism.** Every "highlighted" character in the
product mocks is a hand-written `<Tok kind="…">` span.

## Develop

```bash
npm install
npm run dev          # http://localhost:3000
npm run build        # production build
npm run lint         # ESLint
npx tsc --noEmit     # type-check
```

## Layout

```
src/
  app/
    layout.tsx              # Geist fonts, metadata, Analytics
    page.tsx                # Composes the sections
    globals.css             # Theme tokens + accordion keyframes
    opengraph-image.tsx     # Static OG card for /
    og/route.tsx            # Dynamic OG: /og?title=&subtitle=
  components/
    sections/               # Page sections (Nav, Hero, Features, Wall, Faq, Cta, Footer)
    mocks/                  # Hand-coded product mocks (MacWindow, EditTuiMock, …)
    ui/                     # shadcn-style primitives
    icons/                  # Inline brand marks lucide doesn't ship
    Reveal.tsx              # Motion viewport-reveal wrapper
  lib/
    utils.ts                # cn() (clsx + tailwind-merge)
```

## Mock conventions

- Wrap every mock in `<MacWindow>` (chrome with 3 traffic lights + optional
  title).
- Inside, render diff lines with `<CodeLine kind="plus|minus|scope|ctx|hunk|header" lineNo={…}>`.
- Inside each line, wrap tokens with `<Tok kind="keyword|fn|type|string|number|comment|punct|prop|ident|plain|dim">`.
- Keep all hex colors in `globals.css` `@theme`. Don't hand-write Tailwind
  arbitrary colors in mocks.
- For panes that risk overflow, use:
  `min-w-0 overflow-hidden [mask-image:linear-gradient(to_right,black_94%,transparent)]`
  so any clip looks intentional.

## Deploy (Vercel)

Set the project root to `web/` in the Vercel UI; everything else is default.
No GitHub Action is wired for deploys — Vercel's git integration handles
preview + production deploys.

A separate `web-ci.yml` workflow under `.github/workflows/` runs build +
lint + type-check on every PR that touches `web/`.
