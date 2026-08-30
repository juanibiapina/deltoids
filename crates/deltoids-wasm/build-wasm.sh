#!/usr/bin/env bash
# Build the deltoids wasm engine for the browser PR reviewer.
#
# Requires wasi-sdk (bundles clang + a wasi-libc sysroot) so the tree-sitter C
# grammars compile for wasm. Point WASI_SDK at an extracted release, e.g.
#   https://github.com/WebAssembly/wasi-sdk/releases
#
# Usage:
#   WASI_SDK=/path/to/wasi-sdk-XX.0-<arch>-<os> ./crates/deltoids-wasm/build-wasm.sh
set -euo pipefail

WASI_SDK="${WASI_SDK:-/tmp/wasi-sdk-34.0-arm64-macos}"
if [[ ! -x "$WASI_SDK/bin/clang" ]]; then
  echo "wasi-sdk not found at $WASI_SDK; set WASI_SDK to an extracted release." >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TARGET=wasm32-wasip1
export CC_wasm32_wasip1="$WASI_SDK/bin/clang"
export CFLAGS_wasm32_wasip1="--sysroot=$WASI_SDK/share/wasi-sysroot"

echo "Building deltoids-wasm for ${TARGET}..."
cargo build -p deltoids-wasm --profile wasm --target "$TARGET"

BUILT="target/$TARGET/wasm/deltoids_wasm.wasm"
DEST="$REPO_ROOT/site/public/review/deltoids_wasm.wasm"

if command -v wasm-opt >/dev/null; then
  echo "Optimizing with wasm-opt -Oz..."
  wasm-opt -Oz \
    --enable-bulk-memory --enable-bulk-memory-opt \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    --strip-debug --strip-producers "$BUILT" -o "$DEST"
else
  echo "wasm-opt not found; copying unoptimized wasm." >&2
  cp "$BUILT" "$DEST"
fi
echo "Wrote $DEST ($(du -h "$DEST" | cut -f1))"
