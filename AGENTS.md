# Agent Instructions

## Project Overview

This is a Rust workspace with CLI tools that trace file edits, plus a TUI to browse traces.

**Crates:**
- `edit-cli` — `edit`, `write`, and `edit-tui` CLI commands, plus core trace management library
- `deltoids` — diff library with tree-sitter scope context
- `deltoids-cli` — `deltoids` diff filter CLI
- `tests` — cross-crate integration tests

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

Install binaries locally:
```bash
cargo install --path crates/edit-cli     # edit, write, edit-tui
cargo install --path crates/deltoids-cli  # deltoids
```

## Code Structure

```
crates/
  edit-cli/
    src/lib.rs          # Core library: trace logic, request types, edit/write execution
    src/tui.rs          # TUI rendering and event handling
    src/highlight.rs    # Syntax highlighting for diffs
    src/bin/edit.rs     # Edit CLI binary
    src/bin/write.rs    # Write CLI binary
    src/bin/edit-tui.rs # TUI binary

  deltoids/
    src/lib.rs          # Library exports
    src/parse.rs        # Git diff parsing
    src/scope.rs        # Tree-sitter scope detection and hunk construction
    src/render.rs       # Diff rendering
    src/intraline.rs    # Within-line diff algorithm
    src/reverse.rs      # Diff reversal
    src/syntax.rs       # Language detection and tree-sitter setup

  deltoids-cli/
    src/main.rs         # Standalone diff filter CLI

  tests/
    tests/tui_cli.rs    # Integration tests for edit + write + edit-tui interaction
```

## Website

Marketing/landing site for `deltoids.dev`, built with Astro + Starlight.

```
website/
  astro.config.mjs        # Astro + Starlight configuration
  src/content/docs/       # Markdown/MDX pages (Starlight content collection)
  public/                 # Static assets served as-is
```

Local dev (from `website/`): `npm install`, `npm run dev` (`:4321`), `npm run build`, `npm run preview`.

## Key Patterns

- **Tree-sitter scope context**: Diffs expand hunks to show enclosing functions/classes. Configuration in `deltoids/src/scope.rs` (`MAX_SCOPE_LINES = 200`).
- **Diff computation**: `deltoids::Diff::compute()` parses both old and new files, uses diff-op-aware scope lookup.
- **TUI layout**: Three-pane lazygit-inspired layout (entries, traces, diff).
- **Traces**: Stored in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`.

## Conventions

- Run `cargo fmt --all` before committing.
- Clippy is report-only for now (no `-D warnings`).
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
