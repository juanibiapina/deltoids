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
      engine.ts             #   wasm loader + renderFile/renderFromPatch (theme arg)
      github.ts             #   GitHub REST client + loadSides / renderSides
      github.test.ts        #   renderSides theme + re-render-from-cache tests
      themes.ts             #   curated registry theme names + mode defaults
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
      usePrefs.ts           #   wrap + text-size + chrome + syntax-theme + hide-viewed prefs
      usePrefs.test.ts      #   syntax-theme derivation / persistence tests
      useReviewed.ts        #   per-file "Viewed" state (per-PR blob-sha map)
      useReviewed.test.ts   #   sha-match / reset / toggle / clear tests
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
- Syntax theme is a separate `usePrefs` pref persisted under
  `deltoids.review.syntax-theme`. When unset it derives from the chrome mode
  (dark → Tokyo Night, light → GitHub via `core/themes.ts`); an explicit choice
  from the toolbar `<select>` (grouped Dark/Light, "Auto" clears it) wins and
  persists. The name is passed to the wasm engine as the trailing `theme` arg.
  Switching must not re-hit GitHub: `github.ts` splits `loadSides` (fetch once,
  cached per `FileCard` in a ref) from the pure `renderSides(engine, sides,
  theme)`; a `FileCard` `useEffect` keyed on the theme re-runs only
  `renderSides` from the cached `Sides`. Curated names in `themes.ts` must stay
  valid registry names (`deltoids::theme_names`, i.e. two-face's `as_name()`
  strings plus `TokyoNight`).
- Row line numbers are a `usePrefs` pref persisted under
  `deltoids.review.hide-ln` (default hidden; only an explicit `"0"` shows
  them). It is CSS-only: `App.tsx` adds `hide-ln` to `<main>`, and
  `main.hide-ln .row .ln { display: none }` drops the gutter on every row so
  columns stay aligned. Line numbers then live only in the hunk headers —
  `.lineno` (scope-less) and `.crumb-lineno` (the hunk start number added to
  breadcrumb headers in `render_html.rs`, shared with `deltoids serve`).
- "Viewed" (reviewed) state marks a file done so it stops drawing the eye.
  `useReviewed(ref, files)` stores a per-PR map `{ filename: blobSha }` in
  `localStorage` under `deltoids.review.viewed:${owner}/${repo}/${number}`
  (JSON; a corrupt value is read as empty). A file counts reviewed only while
  its stored sha equals the current `file.sha` (the content-addressed blob sha
  the `/pulls/{n}/files` API returns, now typed on `PrFile`), so a new commit
  that changes the file auto-unmarks only that file — GitHub/Bitbucket reset
  semantics at file granularity. A reviewed card keeps a per-file `Viewed`
  checkbox in its header, gets the `reviewed` class, and CSS collapses the diff
  and slims/mutes the header (`.file.reviewed`); the sidebar row dims and its
  A/M/D/R letter becomes a check. The card stays mounted so sidebar jumps still
  land. `.pr-meta` shows an "N of M reviewed" line with a Clear button. A
  toolbar toggle (`usePrefs.hideViewed`, key `deltoids.review.hide-viewed`,
  **on by default**; only an explicit `"0"` shows them) adds `hide-viewed` to
  `<main>` so `main.hide-viewed .file.reviewed { display: none }` removes
  reviewed cards from the column entirely (sidebar still lists them).
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
