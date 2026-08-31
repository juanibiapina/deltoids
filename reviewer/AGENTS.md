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
      filetree.ts           #   flat PR file list -> grouped tree (tree.rs mirror)
      filetree.test.ts      #   grouping tests ported from tree.rs
      cardHeight.ts         #   estimate a card's height from changed-line counts
      cardHeight.test.ts    #   estimate tests
      vendor/               #   vendored @bjorn3/browser_wasi_shim 0.4.2 + .d.ts
    components/
      Topbar.tsx            #   brand, PR form, token button, toolbar
      FileTree.tsx          #   grouped, collapsible tree (react-accessible-treeview)
      fileIcons.ts          #   filename -> per-type brand icon (simple-icons)
      useFileNavigation.ts  #   pin a clicked file under the topbar through lazy loads
      ReviewView.tsx        #   PR meta + lazy file cards
      FileCard.tsx          #   one lazily-rendered file diff
      LazyObserver.tsx      #   shared IntersectionObserver for lazy cards
      components.test.tsx   #   component tests
    hooks/
      usePrefs.ts           #   wrap + text-size + theme prefs (localStorage)
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
- Theme is a `usePrefs` pref persisted under `deltoids.review.theme`
  (`dark`/`light`); first visit follows `prefers-color-scheme`. `App.tsx` sets
  `data-theme` on `<html>`; the light palette lives in `styles/style.css` under
  `:root[data-theme="light"]` (dark is the plain `:root` default). An inline
  script in `index.html` applies the theme before first paint to avoid a flash —
  keep its `localStorage` key in sync with `usePrefs.ts`.
- The sidebar is a grouped, collapsible file tree (phase 2) built on
  `react-accessible-treeview`. Grouping/sort/collapse mirror the CLI's
  `crates/deltoids-cli/src/sidebar/tree.rs`, which stays the canonical
  cross-check for `filetree.ts`. No virtualization yet (deferred; the tree is
  fully expanded by default). File rows show per-type brand icons (`fileIcons.ts`,
  tree-shaken from `simple-icons`) and a trailing A/M/D/R status letter.
- Clicking a file must land it under the sticky topbar and keep it there while
  cards render lazily. Two things cooperate: skeletons reserve an estimated
  height (`cardHeight.ts`) so the layout barely shifts, and sticky chrome sets
  `overflow-anchor: none` so the browser's native scroll anchoring anchors to
  diff content. That is not enough on its own (the boundary card straddling the
  topbar defeats anchoring, and large diffs finish rendering seconds later), so
  `useFileNavigation` pins the clicked file with a per-frame `requestAnimationFrame`
  loop that re-aligns it until the user scrolls (detected via a `scroll` listener
  that ignores the loop's own scrolls).
