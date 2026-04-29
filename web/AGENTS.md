# Agent notes for `web/`

## Stack invariants

- **Next.js 15 App Router** with Turbopack. Server Components by default; mark
  `"use client"` only where needed (Nav scroll listener, Accordion, Reveal).
- **Tailwind v4**. No `tailwind.config.{ts,js}`. All theme tokens live in
  `src/app/globals.css` under `@theme`.
- **No Shiki, no Prism, no screenshots.** Every "syntax highlighted" piece of
  code on the page is built from `<Tok kind="…">` spans inside `<CodeLine>`.

## Where things live

```
src/components/sections/   # Top-level page sections (each ~one screenful)
src/components/mocks/      # Hand-coded product mocks
src/components/ui/         # shadcn-style primitives
src/components/icons/      # Inline brand marks (lucide-react dropped brands)
src/components/Reveal.tsx  # Motion fade-in-on-view wrapper
```

When adding a new mock:
1. Wrap in `<MacWindow title="…">`.
2. Compose `<CodeLine kind="…">` children for each visible line.
3. Tokenize with `<Tok kind="…">…</Tok>`. Use existing kinds before adding new ones.
4. If content is wider than the column, add
   `min-w-0 overflow-hidden [mask-image:linear-gradient(to_right,black_94%,transparent)]`
   on the inner pane so clip looks intentional.

When adding a new feature section:
- Use `<Feature eyebrow title description bullets align="left|right" alt mock />`.
- Alternate `align` for striping; alternate `alt` for background variation.

## OG cards

- `src/app/opengraph-image.tsx` is the static card for `/`.
- `src/app/og/route.tsx` is a dynamic endpoint: `/og?title=…&subtitle=…`.
- Satori (`@vercel/og`) requires every `<div>` with more than one child to set
  `display: 'flex' | 'contents' | 'none'`. Forgetting this returns
  `failed to pipe response`.

## Common mistakes

- `<Tok kind="comment">// foo</Tok>` is parsed as a JSX comment by ESLint.
  Wrap the literal: `<Tok kind="comment">{"// foo"}</Tok>`.
- `&apos;` inside a JS string literal renders literally (`What&apos;s …`).
  Use a real apostrophe inside strings; only use `&apos;` inside JSX text.
- lucide-react has no `Github` icon. Use `@/components/icons/GitHubIcon`.
- JSX collapses whitespace between `</code>` and following text on a new
  line. If you see "writetools" stuck together, add an explicit `{" "}`.

## Verifying changes

```bash
npm run build        # must pass
npm run lint         # must pass
npx tsc --noEmit     # must pass
```

Visual check with `browse` skill at `http://localhost:3000` — scroll all
sections; check both OG endpoints render valid PNGs.
