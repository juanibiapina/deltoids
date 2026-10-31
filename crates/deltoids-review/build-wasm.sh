#!/usr/bin/env bash
# Build the shared review core for the browser reviewer.
#
# Pure Rust, so plain cargo with the wasm32-unknown-unknown target is enough.
# Writes DEST (default reviewer/public/deltoids_review.wasm).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TARGET=wasm32-unknown-unknown
cargo build -q -p deltoids-review --profile wasm --target "$TARGET"

DEST="${DEST:-$REPO_ROOT/reviewer/public/deltoids_review.wasm}"
mkdir -p "$(dirname "$DEST")"
cp "target/$TARGET/wasm/deltoids_review.wasm" "$DEST"
