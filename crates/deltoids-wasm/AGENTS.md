# deltoids-wasm

A `cdylib` that exposes the `deltoids` engine to the browser reviewer in
`reviewer/` over a small C-ABI. It uses no wasm-bindgen, so the module runs
under a plain WASI shim (`@bjorn3/browser_wasi_shim`).

## C-ABI

- `alloc(len) -> ptr` and `dealloc(ptr, len)` manage buffers. Free every
  `alloc` and every render result with the same `len`.
- `render_file(before*, after*, path*, theme*) -> u64` renders a full
  before/after pair.
- `render_from_patch(after*, patch*, path*, theme*) -> u64` rebuilds the
  before side from a unified patch (GitHub's per-file `patch`), so the client
  fetches only the head content.
- `render_context(after*, path*, theme*, start, end) -> u64` renders new-file
  lines `start..=end` (1-based, inclusive) of `after` as highlighted context
  rows. The reviewer uses it to expand gaps.

Each `*` is a `(ptr, len)` pair. `theme` is a registry name from
`deltoids::theme_names`. An empty name selects the default theme.

Each render returns the HTML bytes as a packed `ptr << 32 | len` handle,
which is valid only on 32-bit wasm. The `extern "C"` functions are thin
marshalling shells over the safe core (`render_html`,
`render_html_from_patch`). Tests target the safe core.

## Build constraints

The `deltoids` crate handles these with `cfg(target_arch = "wasm32")` in its
`Cargo.toml`, `config.rs`, and `language.rs`:

- The C tree-sitter grammars need a libc. The build targets `wasm32-wasip1`
  and points the `cc` crate at the clang and sysroot of wasi-sdk
  (`CC_wasm32_wasip1`, `CFLAGS_wasm32_wasip1`).
- On wasm, syntect uses the pure-Rust `fancy-regex` engine. Native builds
  use oniguruma. Both targets read their assets from the embedded syntect
  dumps of `two-face`.
- Both targets share one theme registry (`config.rs`). `build.rs` converts the
  vendored `assets/themes/*.tmTheme` files to syntect dumps at build time, so
  the wasm module has no runtime plist parser.

## Build

```bash
# Needs wasi-sdk (https://github.com/WebAssembly/wasi-sdk/releases) and,
# optionally, wasm-opt (binaryen).
WASI_SDK=/path/to/wasi-sdk-34.0-<arch>-<os> ./build-wasm.sh
```

`build-wasm.sh` builds with the `wasm` profile, runs `wasm-opt -Oz`, and
copies the result to `DEST` (default `reviewer/public/deltoids_wasm.wasm`).
The `.wasm` file is a gitignored build product.

## Binary size

The module is about 18 MB uncompressed. The tree-sitter grammars make up
most of it. Do not trim the two-face syntax dump: it is under 1 MB, and it
highlights languages that have no tree-sitter grammar.
