# reviewer

Standalone React PR reviewer, deployed to Cloudflare Pages at
`review.deltoids.dev`. It renders any public GitHub pull request as a clean,
scope-expanded diff entirely in the browser, using the `deltoids-wasm` engine.
No backend.

It lives in the monorepo (so it and `crates/deltoids-wasm` change atomically)
but ships independently of the marketing site (`site/`, GitHub Pages).

## Stack

- Vite + React 18 + TypeScript, Vitest (jsdom) for tests.
- The diff engine is `crates/deltoids-wasm`, built to
  `reviewer/public/deltoids_wasm.wasm` (gitignored; a CI/build product).

## Layout

```
reviewer/
  index.html                # Vite entry; mounts React into #root
  vite.config.ts            # base "/", React plugin, Vitest config
  public/
    deltoids_wasm.wasm       # engine (gitignored, built by build-wasm.sh)
  src/
    main.tsx                # React root + imports the stylesheet
    App.tsx                 # app shell: review flow, deep-link, token, prefs
    core/                   # framework-neutral, DOM-free logic
      engine.ts             #   wasm loader + renderFile/renderFromPatch
      github.ts             #   GitHub REST client + per-file rendering
      lib.ts                #   pure helpers (parsePrUrl, base64, badgeClass)
      lib.test.ts           #   Vitest unit tests for lib.ts
      vendor/               #   vendored @bjorn3/browser_wasi_shim 0.4.2 + .d.ts
    components/
      Topbar.tsx            #   brand, PR form, token button, toolbar
      Intro.tsx             #   first-run empty state
      Sidebar.tsx           #   flat file list (phase 1)
      ReviewView.tsx        #   PR meta + lazy file cards
      FileCard.tsx          #   one lazily-rendered file diff
      LazyObserver.tsx      #   shared IntersectionObserver for lazy cards
      components.test.tsx   #   component tests
    hooks/
      usePrefs.ts           #   wrap + text-size prefs (localStorage)
      useTopbarHeight.ts    #   --topbar-h sync via ResizeObserver
    styles/style.css        # the reviewer stylesheet (deltoids HTML contract)
```

## Dev / build / test

```bash
cd reviewer
npm install
# Build the engine once (needs wasi-sdk; see crates/deltoids-wasm/AGENTS.md):
DEST="$PWD/public/deltoids_wasm.wasm" \
  WASI_SDK=/path/to/wasi-sdk ../crates/deltoids-wasm/build-wasm.sh
npm run dev        # http://localhost:5173
npm run build      # tsc --noEmit && vite build -> reviewer/dist/
npm run preview    # serve dist as prod will
npm test           # vitest run
npm run typecheck  # tsc --noEmit
```

The engine fetches from `/deltoids_wasm.wasm` (root), so `base` is `/` and the
app is served from the root of its own subdomain.

## Deploy

`.github/workflows/reviewer.yml` builds the wasm engine (wasi-sdk + wasm-opt),
then runs Vitest + type-check + `vite build`, and deploys `reviewer/dist` to
Cloudflare Pages (project `deltoids-reviewer`). It runs on `reviewer/**`,
`crates/deltoids/**`, or `crates/deltoids-wasm/**` changes.

Required repo secrets: `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`. The
custom domain `review.deltoids.dev` is attached to the Pages project (DNS
`CNAME review -> deltoids-reviewer.pages.dev`).

## Notes

- The GitHub token lives in `localStorage` under `deltoids.gh.token`. It does
  not cross origin, so users re-enter it once on the new subdomain.
- Phase 1 is exact parity with the old flat-list reviewer. The virtualized file
  tree is phase 2 (see the extraction plan).
