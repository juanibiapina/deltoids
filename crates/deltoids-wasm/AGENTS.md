# deltoids-wasm

WebAssembly build of the deltoids diff engine for the browser PR reviewer at
`deltoids.dev/review/`. It exposes the engine over a tiny C-ABI so a static web
app can compute deltoids diffs client-side, with no backend.

## What it is

A `cdylib` that wraps `deltoids` and exports three functions over linear memory
(no wasm-bindgen, so the module runs under a plain WASI shim in the browser):

- `alloc(len) -> ptr` / `dealloc(ptr, len)` — buffer management. Every `alloc`
  and every render result must be freed with the same `len`.
- `render_file(before*, after*, path*) -> u64` — full before/after render.
- `render_from_patch(after*, patch*, path*) -> u64` — reconstructs the before
  side from a unified patch (GitHub's per-file `patch`), so the client fetches
  only the head content.

Each render returns a packed `ptr << 32 | len` handle to the HTML bytes (valid
only on 32-bit wasm). The safe core is `render_html` / `render_html_from_patch`;
the `extern "C"` functions are thin marshalling shells. Tests target the safe
core (the packed pointer is wasm-only).

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

## Build

```bash
# Requires wasi-sdk (bundles clang + a wasi-libc sysroot) and, optionally,
# wasm-opt (binaryen) for size. Download wasi-sdk from:
#   https://github.com/WebAssembly/wasi-sdk/releases
WASI_SDK=/path/to/wasi-sdk-34.0-<arch>-<os> ./build-wasm.sh
```

`build-wasm.sh` builds with the size-tuned `wasm` profile, runs `wasm-opt -Oz`
(bulk-memory features enabled), and copies the result to
`site/public/review/deltoids_wasm.wasm`. CI does the same in `pages.yml`. The
`.wasm` is gitignored; it is a build product.

## The web app

`site/public/review/` is the static reviewer (`index.html`, `app.js`,
`style.css`, vendored `browser_wasi_shim.js`, and DOM-free helpers in `lib.js`
tested by `site/test/review-lib.test.mjs`). It:

- parses a PR URL or `owner/repo/number`, or a `?pr=` deep link;
- calls the GitHub REST API directly (CORS-open), anonymously by default;
- stores an optional read-only PAT in `localStorage` for higher limits and
  private repos (no OAuth — a static site cannot hold a client secret);
- renders each changed file through the wasm engine, reusing deltoids' HTML
  class contract and the `serve` diff CSS.

## Known follow-up

The wasm is ~15 MB (~3 MB gzipped) because two-face embeds ~200 languages.
Trimming to deltoids' ~19 languages needs a syntax subset built from source (the
compiled two-face set cannot be filtered — its contexts cross-reference by
index).
