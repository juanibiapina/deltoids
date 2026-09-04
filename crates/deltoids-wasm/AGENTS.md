# deltoids-wasm

WebAssembly build of the deltoids diff engine for the browser PR reviewer at
`review.deltoids.dev` (the standalone React app in `reviewer/`). It exposes the
engine over a tiny C-ABI so a static web app can compute deltoids diffs
client-side, with no backend.

## What it is

A `cdylib` that wraps `deltoids` and exports three functions over linear memory
(no wasm-bindgen, so the module runs under a plain WASI shim in the browser):

- `alloc(len) -> ptr` / `dealloc(ptr, len)` — buffer management. Every `alloc`
  and every render result must be freed with the same `len`.
- `render_file(before*, after*, path*, theme*) -> u64` — full before/after
  render.
- `render_from_patch(after*, patch*, path*, theme*) -> u64` — reconstructs the
  before side from a unified patch (GitHub's per-file `patch`), so the client
  fetches only the head content.
- `render_context(after*, path*, theme*, start, end) -> u64` — renders new-file
  lines `start..=end` (1-based inclusive scalars) of `after` as highlighted
  context rows, backing the reviewer's gap expansion (revealing the unshown
  lines a `.gap` divider stands in for). The highlight syntax is detected from
  `path`.

Each `*` is a `(ptr, len)` pair. The trailing `theme` pair is a `deltoids`
registry theme name (see `deltoids::theme_names`); pass `(_, 0)` / an empty
string to use the default (Monokai Extended on wasm). `engine.ts` and the React
app own the theme choice (a persisted `localStorage` pref); the wasm side just
takes the name and resolves it through the registry, so switching themes
re-renders from cached content with no re-fetch.

Each render returns a packed `ptr << 32 | len` handle to the HTML bytes (valid
only on 32-bit wasm). The safe core is `render_html` / `render_html_from_patch`
(both take the theme name as a plain `&str`); the `extern "C"` functions are
thin marshalling shells. Tests target the safe core (the packed pointer is
wasm-only), including one asserting a theme name changes colors but not row
structure.

## How the engine builds for wasm

The engine reuses the whole `deltoids` crate — tree-sitter scopes, intraline
emphasis, syntect highlighting. Two constraints shape the build, both handled in
`deltoids`'s `Cargo.toml` and `config.rs`/`language.rs` via
`cfg(target_arch = "wasm32")`:

- **C grammars need a libc.** `wasm32-unknown-unknown` ships none, so the C
  tree-sitter grammars fail to compile. The build targets `wasm32-wasip1` and
  points the `cc` crate at wasi-sdk's clang + sysroot (`CC_wasm32_wasip1`,
  `CFLAGS_wasm32_wasip1`). The browser runs the wasip1 module under
  `@bjorn3/browser_wasi_shim`.
- **No oniguruma, no bat, no filesystem.** On wasm, syntect uses the pure-Rust
  `fancy-regex` engine and assets come from `two-face`'s embedded syntect dumps
  instead of bat. Native builds are unchanged (onig + bat).
- **One theme registry, both targets.** `deltoids`'s `config.rs` builds a
  name-keyed `theme_by_name` / `theme_names` registry — from bat's themes on
  native, from two-face's embedded themes on wasm — plus a vendored Tokyo Night
  (`crates/deltoids/assets/themes/tokyonight.tmTheme`) that `deltoids`'s
  `build.rs` converts to a syntect dump at build time and both targets
  `include_bytes!` + `from_binary` (via `dump-load`, no runtime plist parser).
  The `plist-load`/`dump-create` build cost is a host-only build-dependency, so
  resolver v2 keeps it out of the wasm module.

## Build

```bash
# Requires wasi-sdk (bundles clang + a wasi-libc sysroot) and, optionally,
# wasm-opt (binaryen) for size. Download wasi-sdk from:
#   https://github.com/WebAssembly/wasi-sdk/releases
WASI_SDK=/path/to/wasi-sdk-34.0-<arch>-<os> ./build-wasm.sh
```

`build-wasm.sh` builds with the size-tuned `wasm` profile, runs `wasm-opt -Oz`
(bulk-memory features enabled), and copies the result to `DEST` (default
`reviewer/public/deltoids_wasm.wasm`). CI does the same in `reviewer.yml`. The
`.wasm` is gitignored; it is a build product.

## The web app

`reviewer/` is the standalone React reviewer (Vite + TypeScript). Its
framework-neutral core lives in `reviewer/src/core/` (`engine.ts` wasm loader,
`github.ts` REST client, `lib.ts` DOM-free helpers with `lib.test.ts`, and the
vendored `browser_wasi_shim.js`); the UI is in `reviewer/src/components/`. See
`reviewer/AGENTS.md`. It:

- parses a PR URL or `owner/repo/number`, or a `?pr=` deep link;
- calls the GitHub REST API directly (CORS-open), anonymously by default;
- stores an optional read-only PAT in `localStorage` for higher limits and
  private repos (no OAuth — a static site cannot hold a client secret);
- renders each changed file through the wasm engine, reusing deltoids' HTML
  class contract and the `serve` diff CSS.

The layout is responsive over a media-query ladder (640 / 1024 / 1440px) driven
by CSS variables. The file sidebar is a persistent grid column at ≥1024px and an
off-canvas drawer below (the "Files" button toggles `body.drawer-open`; the DOM
and IntersectionObserver stay put). `.file-head` is sticky, offset by a live
`--topbar-h` that `useTopbarHeight` recomputes from the real topbar with a
`ResizeObserver` so the offset tracks the form wrapping on phones. A toolbar
carries a wrap toggle (`main.nowrap` gives container-level horizontal scroll with
a sticky-left gutter) and a text-size cycle (`main[data-size]`); both persist in
`localStorage`. The PR input is ≥16px to stop iOS focus-zoom, touch targets are
≥44px, and safe-area insets are honored.

## Binary size

The module is ~18 MB uncompressed, dominated by the tree-sitter parser tables
(the C++, Ruby, and Bash grammars alone are the bulk); the embedded two-face
syntax dump is a zlib-compressed <1 MB `include_bytes!`, so trimming its ~200
languages would save under a megabyte and is not worth doing — syntect also
highlights any language a PR touches independently of the ~19 tree-sitter
grammars, so trimming would drop that highlighting. The bytes users download are
already small: Cloudflare Pages serves the `.wasm` Brotli-compressed (~2.2 MB),
which `WebAssembly.instantiateStreaming` decodes transparently. Cutting the
uncompressed size further means dropping grammars, which loses scope context for
those languages.
