# Agent Instructions

## Project Overview

This is a Rust workspace with CLI tools that trace file edits, plus a TUI to browse traces.

**Crates:**
- `edit-cli` — `edit`, `write`, and `edit-tui` CLI commands, plus core trace management library
- `deltoids` — diff library with tree-sitter scope context. Optional features:
  - `blob-resolve` — adds `git`/`content` modules for resolving before/after blob content from a git repo (used by `deltoids` and `rv` bins).
  - `ratatui` — adds `render_tui` for rendering hunks/headers as `ratatui::text::Line<'static>` (used by `edit-tui` and `rv`).
- `deltoids-cli` — `deltoids` ANSI diff filter CLI
- `rv-cli` — `rv` interactive TUI for scrolling diffs (same input pipeline as `deltoids`, ratatui rendering)
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
cargo install --path crates/rv-cli        # rv
```

## Code Structure

```
crates/
  edit-cli/
    src/lib.rs              # Library entry: request types, edit/write execution
    src/trace_store.rs      # TraceStore: trace dir layout, append/read/list
    src/tui.rs              # edit-tui chrome (panes, lists, HistoryEntry header) — diff lines come from `deltoids::render_tui::render_hunk`
    src/bin/edit.rs         # Edit CLI binary
    src/bin/write.rs        # Write CLI binary
    src/bin/edit-tui.rs     # TUI binary

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
    src/structural.rs       # Structural ("tree-aware") diff layer
    src/structural/symbol.rs    # Per-language Symbol extractor (Function/Method/Class/...)
    src/structural/pair.rs      # Old/new symbol pairing (path match + rename detection)
    src/structural/classify.rs  # ChangeKind classifier + human descriptions
    src/structural/diff.rs      # Top-level StructuralDiff::compute
    src/structural/outline.rs   # File outline (every symbol + per-symbol diff status)
    src/structural/render.rs    # format_summary / format_summary_with(opts) / totals
    tests/diff_cases.rs       # Entry point for the diff-case reference suite
    tests/diff_cases/         # Harness, README, and case directories
      cases/<NNN-slug>/       # 1-case.md, 2-original.<EXT>, 3-updated.<EXT>, 4-expected.diff
    tests/structural_cases.rs # Entry point for the structural-case reference suite
    tests/structural_cases/   # Harness, README, and case directories
      cases/<NNN-slug>/       # 1-case.md, 2-original.<EXT>, 3-updated.<EXT>, 4-expected.structural

  deltoids-cli/
    src/main.rs         # Standalone ANSI diff filter CLI (uses deltoids::{git, content})

  rv-cli/
    src/main.rs         # Interactive scrolling TUI (uses deltoids::{git, content, render_tui})
    src/sidebar.rs      # Lazygit-style file tree sidebar (status badges, icons, deltas)

  tests/
    tests/tui_cli.rs    # Integration tests for edit + write + edit-tui interaction
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
- **Structural diff**: `deltoids::StructuralDiff::compute(original, updated, path)` (also `Diff::structural()`) walks both ASTs via `ParsedFile::root_node()`, extracts `Symbol`s per language (visibility-aware), pairs them by qualified path with a Jaccard signature-similarity rename fallback, and classifies each pair as `Added`/`Removed`/`Modified`/`Renamed`/`SignatureChanged`/`VisibilityChanged`/`BodyChanged`. Bullet glyphs `+`/`-`/`→`/`~`. The classifier suppresses redundant container-modified entries (e.g. "Modified class Foo" is dropped when "Added method Foo::bar" is present). Borrows ideas from difftastic (content-id hashing, name-driven pairing) without the full Dijkstra graph search — line-level diff stays the spine.
- **Structural views**: `deltoids` CLI gets `-s/--summary`, `-S/--summary-then-diff`, `-p/--public`, `--signatures-only`. `rv` cycles `Full → Outline → Summary` with `v` and toggles public-only with `p`; the Outline view (built on `structural::outline`) shows every symbol in the file with a diff-coloured background reflecting status, and the Summary view shows the human change descriptions. `edit-tui` toggles a per-entry summary with `s` (uses each hunk's deepest breadcrumb ancestor since the trace doesn't carry full source).
- **TUI layout**: Three-pane lazygit-inspired layout (entries, traces, diff).
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

## Structural cases (start here when changing the structural diff layer)

`crates/deltoids/tests/structural_cases/` mirrors `diff_cases/` but for
`deltoids::StructuralDiff::compute`. Each case has the same
`1-case.md` / `2-original.<EXT>` / `3-updated.<EXT>` triple, plus a
`4-expected.structural` capturing the `format_summary` output.

When you change anything in the structural layer (`structural/symbol`,
`pair`, `classify`, `diff`, `render`, per-language node tables), follow
the same loop as for `diff_cases`. The single env var
`DELTOIDS_UPDATE_CASES=1` refreshes both suites:

```bash
cargo test -p deltoids --test structural_cases
DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test structural_cases
```

Review every changed `4-expected.structural` by hand. Adding a new
language or node kind almost always wants a new case to pin the
behaviour.

## Conventions

- Run `cargo fmt --all` before committing.
- Clippy is report-only for now (no `-D warnings`).
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
- For diff-engine changes, the diff-case suite is the canonical test
  surface (see above). Inline `#[test]`s in `scope.rs` etc. remain
  useful for narrow property assertions; cases are the broad spec.
