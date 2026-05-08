# Agent Instructions

## Project Overview

This is a Rust workspace with CLI tools that trace file edits, plus a TUI to browse traces.

**Crates:**
- `deltoids` — diff library with tree-sitter scope context. Optional features:
  - `blob-resolve` — adds `git`/`content` modules for resolving before/after blob content from a git repo (used by the `pager` and `review` subcommands).
  - `ratatui` — adds `render_tui` for rendering hunks/headers as `ratatui::text::Line<'static>` (used by the `review` and `traces` subcommands).
- `deltoids-cli` — ships a single `deltoids` binary with subcommands: `pager` (ANSI diff filter), `review` (scrolling TUI), `edit`/`write` (agent edit tools), `traces` (trace browser). Also holds the trace-management library shared by `edit`/`write`. Cargo-dist publishes one homebrew formula (`deltoids`) and one shell installer for this crate.
- `tests` — cross-crate integration tests

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

Install the binary locally:
```bash
cargo install --path crates/deltoids-cli  # produces the `deltoids` binary
```

## Code Structure

```
crates/
  deltoids/
    src/lib.rs              # Library exports
    src/engine.rs           # Line-level diff engine: Snapshot, DiffOp, align_old_to_new
    src/parse.rs            # Git diff parsing
    src/scope.rs            # Hunk types, Hunk::runs / HunkRun, public entry
    src/scope/range.rs      # Planning phase: ContextRange per diff op
    src/scope/hunk_builder.rs # Filling phase: ContextRange -> Hunk
    src/render.rs           # Diff rendering as ANSI strings (used by deltoids CLI)
    src/render_tui.rs       # Diff rendering as ratatui Line<'static>; also exports shared pane chrome helpers (pane_block, pane_block_with_footer, pane_border_color, render_pane_scrollbar) used by edit-tui and rv (feature: ratatui)
    src/git.rs              # libgit2 wrapper for blob lookup (feature: blob-resolve)
    src/content.rs          # Resolve before/after content for a FileDiff (feature: blob-resolve)
    src/intraline.rs        # Within-line diff algorithm
    src/reverse.rs          # Diff reversal
    src/language.rs         # Stable language detection and per-language parser config
    src/syntax.rs           # ParsedFile tree-sitter parsing and scope queries
    tests/diff_cases.rs       # Entry point for the diff-case reference suite
    tests/diff_cases/         # Harness, README, and case directories
      cases/<NNN-slug>/       # 1-case.md, 2-original.<EXT>, 3-updated.<EXT>, 4-expected.diff

  deltoids-cli/
    src/lib.rs              # Library entry: request types and edit/write execution shared by the edit subcommands
    src/trace_store.rs      # TraceStore: trace dir layout, append/read/list
    src/tui.rs              # `traces` subcommand chrome (panes, lists, HistoryEntry header) — diff lines come from `deltoids::render_tui::render_hunk`
    src/sidebar.rs          # Lazygit-style file tree sidebar for `review` (status badges, icons, deltas)
    src/cli.rs              # Subcommand module declarations
    src/cli/pager.rs        # `deltoids pager` (ANSI diff filter; uses deltoids::{git, content})
    src/cli/review.rs       # `deltoids review` (scrolling TUI; uses deltoids::{git, content, render_tui})
    src/cli/edit.rs         # `deltoids edit` subcommand
    src/cli/write.rs        # `deltoids write` subcommand
    src/cli/traces.rs       # `deltoids traces` subcommand (delegates to `tui::run`)
    src/bin/deltoids.rs     # Single binary; clap dispatcher for all subcommands

  tests/
    tests/tui_cli.rs    # Integration tests for `deltoids edit`/`write`/`traces` interaction
```

## Site

Marketing/landing site for `deltoids.dev`, under `site/` (bare Astro,
no integrations, zero client JS). Deploys to GitHub Pages from the
`Pages` workflow on push to `main` when `site/**` changes. Self-hosted
IBM Plex Sans + JetBrains Mono.

Local dev (from `site/`): `npm install`, `npm run dev` (`:4321`),
`npm run build`, `npm run preview`, `npx astro check`.

See `site/AGENTS.md` for component conventions and the release
checklist.

## Key Patterns

- **Tree-sitter scope context**: Diffs expand hunks to show enclosing functions/classes. Configuration in `deltoids/src/scope.rs` (`MAX_SCOPE_LINES = 200`). `deltoids/src/language.rs` owns stable language detection (bundled syntect path/shebang detection), `Language` ids, tree-sitter parser selection, and per-language node-kind tables. The public parsing surface is `ParsedFile` in `deltoids/src/syntax.rs`, which owns the parsed source and exposes `enclosing_scopes(line)`, `is_structure(scope)`, and `is_data(scope)`. Callers never touch raw tree-sitter taxonomy. `promoted_kinds` covers wrappers like `public_field_definition` or `variable_declarator` that count as a structure when their `value` field is a function body (JS/TS class arrow-fields, top-level `const f = () => {}`). `function_body_kinds` both gates promotion and demotes nested helpers (e.g. `fn inner` inside `fn outer`) so they don't steal the outer anchor.
- **Diff computation**: `deltoids::Diff::compute()` first runs `engine::Snapshot::compute()` (line-level diff via `gix-imara-diff` Histogram + line postprocessing) to produce a `Vec<DiffOp>` and unified text. It then detects one stable `Language` from the path plus in-memory snapshots, parses both old and new snapshots with that language, and runs two phases in `scope/`: `range.rs` plans `ContextRange`s per diff op (anchored on enclosing scope or default 3-line fallback), and `hunk_builder.rs` fills each range into a `Hunk` from the diff ops. `engine::align_old_to_new(line, ops)` is the shared helper for mapping OLD line numbers through the diff (used by `same_slot` rename detection).
- **Hunk iteration**: Consumers walk hunks via `Hunk::runs()` -> `HunkRun` (`Header` / `Subhunk` / `Context`) instead of regrouping lines themselves. Both `render::render_hunk` and the TUI's `detail_items` share this iterator; reach for it before adding new line-grouping code.
- **TUI layout**: Three-pane lazygit-inspired layout (entries, traces, diff). Both `deltoids review` and `deltoids traces` use shared pane chrome from `deltoids::render_tui`.
- **Default behavior**: Plain `deltoids` (no subcommand) runs `pager` when stdin is piped (preserving `git config core.pager 'deltoids | less -R'`) and prints help when stdin is a TTY.
- **Traces**: Stored in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`.

## Diff cases (start here when changing the diff engine)

`crates/deltoids/tests/diff_cases/` is both an integration test and a
product reference for `deltoids::Diff::compute`. Each case directory
holds a description, an `original`/`updated` input pair, and an
`expected.diff` recording the engine's output. See
`tests/diff_cases/README.md` for the format.

**Whenever you change the diff engine** (`scope.rs`, `syntax.rs`,
`language.rs`, `parse.rs`, `intraline.rs`, `reverse.rs`, hunk construction, breadcrumb
rules, etc.) follow this loop:

1. Pick the case that matches the behaviour, or add a new one. New cases
   start with `1-case.md` (the explainer), `2-original.<EXT>`, and
   `3-updated.<EXT>`. Keep the inputs minimal.
2. Run the suite to see the impact:
   ```bash
   cargo test -p deltoids --test diff_cases
   ```
   The failure output prints a diff between recorded and actual for every
   moved case.
3. When the new behaviour is what you want, refresh expectations:
   ```bash
   DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test diff_cases
   ```
   Inspect every changed `4-expected.diff` by hand before committing.
   These files are the spec; they should never change quietly.
4. If a case moved in a way you did not intend, fix the implementation
   rather than the expected output.

For brand-new behaviour, add the case **before** the implementation:
the failing case becomes the spec, and the diff between expected (what
you wrote) and actual (what the engine does) drives the change.

## Conventions

- Run `cargo fmt --all` before committing.
- Clippy is report-only for now (no `-D warnings`).
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
- For diff-engine changes, the diff-case suite is the canonical test
  surface (see above). Inline `#[test]`s in `scope.rs` etc. remain
  useful for narrow property assertions; cases are the broad spec.
