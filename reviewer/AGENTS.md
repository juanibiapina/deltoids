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
      engine.ts             #   wasm compile (main) + instantiate/marshal (worker)
      review.worker.ts      #   module worker: engine + Comlink-exposed sessions
      reviewClient.ts       #   main-thread side: starts the worker, openReview
      renderSession.ts      #   per-review fetch + render queue + theme cache
      renderSession.test.ts #   urgency, mid-render priority, cache, context tests
      github.ts             #   GitHub REST client + loadSides / renderSides
      github.test.ts        #   renderSides theme + re-render-from-cache tests
      sidesLoader.ts        #   per-review sides cache + background prefetch queue
      sidesLoader.test.ts   #   queue order, priority, rate budget, failure tests
      idle.ts               #   shared one-task-per-idle-slot queue (whenIdle)
      idle.test.ts          #   ordering + cancel tests
      themes.ts             #   curated registry theme names + mode defaults
      lib.ts                #   pure helpers (parsePrUrl, looksBinary)
      lib.test.ts           #   Vitest unit tests for lib.ts
      filetree.ts           #   flat PR file list -> grouped tree (tree.rs mirror),
                            #     display order, selection membership, row stepping
      filetree.test.ts      #   grouping tests ported from tree.rs + selection
      wheel.ts              #   Ctrl/Shift wheel events -> sidebar steps (scroll.rs mirror)
      wheel.test.ts         #   tick, trackpad stream, kind, direction, deltaMode tests
      overview.ts           #   scrollbar geometry: change cells, thumb, drag
      overview.test.ts      #   tests ported from the TUI diff_scrollbar.rs
      vendor/               #   vendored @bjorn3/browser_wasi_shim 0.4.2 + .d.ts
    components/
      Topbar.tsx            #   brand, PR form, token button, toolbar
      FileTree.tsx          #   grouped, collapsible tree (react-accessible-treeview)
      fileIcons.ts          #   filename -> per-type brand icon (simple-icons)
      ReviewView.tsx        #   selection + diff pane of lazy file cards
      DiffScrollbar.tsx     #   the diff pane's scrollbar with added/removed marks
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
  Switching must not re-hit GitHub: fetching (`loadSides`, cached by the
  review's `SidesLoader`) is split from the pure `renderSides(engine, sides,
  theme)`, and the worker's `RenderSession` re-renders from the `Sides` it
  already holds. Curated names in `themes.ts` must stay
  valid registry names (`deltoids::theme_names`, i.e. two-face's `as_name()`
  strings plus `TokyoNight`).
- Row line numbers are a `usePrefs` pref persisted under
  `deltoids.review.hide-ln` (default hidden; only an explicit `"0"` shows
  them). It is CSS-only: `App.tsx` adds `hide-ln` to `<main>`, and
  `main.hide-ln .row .ln { display: none }` drops the gutter on every row so
  columns stay aligned. Line numbers then live only in the hunk headers —
  `.lineno` (scope-less) and `.crumb-lineno` (the hunk start number added to
  breadcrumb headers in `render_html.rs`).
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
  change letter becomes a check. A
  toolbar toggle (`usePrefs.hideViewed`, key `deltoids.review.hide-viewed`,
  **on by default**; only an explicit `"0"` shows them) adds `hide-viewed` to
  `<main>` so `main.hide-viewed .file.reviewed { display: none }` removes
  reviewed cards from the column entirely (sidebar still lists them).
- The sidebar is a grouped, collapsible file tree (phase 2) built on
  `react-accessible-treeview`. Grouping/sort/collapse mirror the CLI's
  `crates/deltoids-cli/src/sidebar/tree.rs`, which stays the canonical
  cross-check for `filetree.ts`. No virtualization yet (deferred; the tree is
  fully expanded by default). File rows mirror the TUI sidebar row: an A/M/D/R letter in the chevron
  column (coloured like the TUI), a per-type brand icon (`fileIcons.ts`,
  tree-shaken from `simple-icons`), the name, then `+N -M` from GitHub's
  `additions`/`deletions` (zero counts left out).
- The diff column shows one selection at a time, like the TUI diff pane:
  one file, or every file under a directory row the user clicks (a dir's
  chevron only folds it). `Selection` lives in `ReviewView` state (not the
  tree library's `selectedIds`, which would fight the prune-remount `key`)
  and starts on the first file in tree order. Cards render in tree display
  order (`filetree.ts::displayOrder`) and all stay mounted; unselected ones get
  `hidden`, so the fetched sides, theme re-render, and expanded gaps survive
  switching, and `display: none` keeps them out of the lazy loader. Every
  selection change scrolls the window to the top. A single selected file gets
  `solo`, which keeps it on screen when marked viewed under hide-viewed.
- Ctrl+wheel or Shift+wheel over the diff pane steps the selection one
  sidebar row at a time, like the TUI. `ReviewView` listens on `.pane` with
  a non-passive `wheel` listener so `preventDefault` blocks page zoom and
  sideways scroll (a trackpad pinch arrives as Ctrl+wheel, so it steps too).
  `core/wheel.ts` turns events into steps; `filetree.ts::stepSelection`
  picks the next shown row, skipping pruned files and folded directories.
  Fold state stays inside the tree library; `FileTree` reports each change
  through `onExpandChange` and `ReviewView` mirrors it in a ref. The
  library's controlled `expandedIds` was rejected: collapsing a directory
  also folds its descendants internally, out of sync with the prop.
- Content fetching and wasm rendering run in one module worker
  (`core/review.worker.ts`, RPC through Comlink), so a render never blocks
  input or paint. The page keeps `fetchPr`/`fetchFiles` (the tree needs
  them) and passes the file list, shas, tree display order, and token to
  `reviewClient.ts::openReview`, which returns that review's
  `RenderSession` (`core/renderSession.ts`). Workers cannot read
  `localStorage`, so the token crosses at `openReview` and on
  `setToken`. The session wraps `core/sidesLoader.ts::createSidesLoader`:
  background fetches in tree order through `p-queue` (concurrency 4), one
  cached promise per file, and `ReviewView` moves the current selection to
  the front with `prioritize`. Background fetching stops when
  `github.ts::rateRemaining()` (the worker's last seen
  `x-ratelimit-remaining`) is at or below 10 or a content request throws
  `RateLimitError`; urgent loads always run, and a failed background fetch
  is retried by the next urgent one. Renders run one at a time, urgent
  (`"now"`, from a shown card the lazy observer has seen) ahead of
  background, with gap context rows above both. A wasm call cannot be
  interrupted, so the session yields a macrotask after each one; without the
  yield, queued background renders would run before a click's request is
  even read. A click still waits for the render in flight. Each file
  keeps HTML for one theme; a request for another theme supersedes it.
  `FileCard` drops results for a theme that is no longer current, paints
  urgent results at once, and applies background results in a
  `core/idle.ts::whenIdle` slot (the `innerHTML` cost stays on the main
  thread), so hidden cards are ready before they are selected. The engine
  download starts on page load: `App` calls `startReviewWorker()` on mount,
  which runs `compileStreaming` on the main thread against the
  `index.html` preload of `/deltoids_wasm.wasm` (`as="fetch"` +
  `crossorigin`; keep them matched or it downloads twice) and posts the
  compiled module to the worker to instantiate.
- While a review is open the page does not scroll: `App.tsx` adds
  `reviewing` to `<html>`, the app fills the window, and the diff column is a
  bordered `.pane` whose `.pane-scroll` child scrolls (native scrollbar
  hidden), like the TUI diff pane. File headers scroll with the diff and
  mirror the TUI's: bold path, a separator rule, and a muted `renamed:` line.
  `LazyObserverProvider` takes the pane as its IntersectionObserver `root`,
  created on the first registration so the ref is already attached.
  `DiffScrollbar` sits inside the pane's right edge and maps the pane's
  scroll. It measures the shown `.row.added`/`.row.removed` boxes when the
  pane or its content resizes or the theme changes, and paints
  `core/overview.ts::changeCells` to a canvas; scroll only moves the thumb.
  Track click and thumb drag set the pane's `scrollTop`. jsdom has no layout,
  so only `overview.ts` is unit-tested.
