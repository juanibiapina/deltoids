# Agent Instructions

## Project Overview

This is a Rust workspace with CLI tools that trace file edits, plus a TUI to browse traces.

**Crates:**
- `edit` (root) — `edit` and `write` CLI commands, `edit-tui` browser
- `deltoids` — diff filter with tree-sitter scope context, usable standalone or as a library

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

Install binaries locally:
```bash
cargo install --path .
cargo install --path deltoids
```

## Code Structure

```
src/
  main.rs        # CLI entry (edit, write, edit-tui subcommands)
  tui.rs         # TUI rendering and event handling
  highlight.rs   # Syntax highlighting for diffs

deltoids/
  src/
    main.rs      # Standalone diff filter CLI
    lib.rs       # Library exports
    parse.rs     # Git diff parsing
    scope.rs     # Tree-sitter scope detection and hunk construction
    render.rs    # Diff rendering
    intraline.rs # Within-line diff algorithm
    reverse.rs   # Diff reversal
```

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
