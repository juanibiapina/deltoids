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
    src/lib.rs              # Library entry: request types, edit/write execution
    src/trace_store.rs      # TraceStore: trace dir layout, append/read/list
    src/theme.rs            # TUI theme resolution (syntax + UI colors)
    src/tui.rs              # TUI rendering and event handling
    src/highlight.rs        # Syntax highlighting for diffs
    src/bin/edit.rs         # Edit CLI binary
    src/bin/write.rs        # Write CLI binary
    src/bin/edit-tui.rs     # TUI binary

  deltoids/
    src/lib.rs              # Library exports
    src/parse.rs            # Git diff parsing
    src/scope.rs            # Hunk types, Hunk::runs / HunkRun, public entry
    src/scope/range.rs      # Planning phase: ContextRange per diff op
    src/scope/hunk_builder.rs # Filling phase: ContextRange -> Hunk
    src/render.rs           # Diff rendering
    src/intraline.rs        # Within-line diff algorithm
    src/reverse.rs          # Diff reversal
    src/syntax.rs           # Language detection, ParsedFile, scope taxonomy
    tests/diff_cases.rs       # Entry point for the diff-case reference suite
    tests/diff_cases/         # Harness, README, and case directories
      cases/<NNN-slug>/       # 1-case.md, 2-original.<EXT>, 3-updated.<EXT>, 4-expected.diff

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
  SCREENSHOTS.md          # Recipes for reproducing landing-page screenshots
```

Local dev (from `website/`): `npm install`, `npm run dev` (`:4321`), `npm run build`, `npm run preview`.

Landing-page screenshots: see `website/SCREENSHOTS.md` for capture recipes.

## Web (Next.js, parallel)

A second landing site lives under `web/` (Next.js 15 App Router, Tailwind
v4, shadcn primitives, Motion, `@vercel/og`). It deploys to Vercel and uses
**hand-coded React product mocks** instead of screenshots. Astro under
`website/` keeps deploying to GitHub Pages; the two coexist until a
decision is made.

Local dev (from `web/`): `npm install`, `npm run dev` (`:3000`),
`npm run build`, `npm run lint`, `npx tsc --noEmit`.

See `web/AGENTS.md` for component conventions.

## Key Patterns

- **Tree-sitter scope context**: Diffs expand hunks to show enclosing functions/classes. Configuration in `deltoids/src/scope.rs` (`MAX_SCOPE_LINES = 200`). The public surface is `ParsedFile` in `deltoids/src/syntax.rs`, which owns the parsed source and exposes `enclosing_scopes(line)`, `is_structure(scope)`, and `is_data(scope)`. Per-language node-kind tables (`structure_kinds`, `data_kinds`, `promoted_kinds`, `function_body_kinds`) are internal config of `ParsedFile` — callers never touch raw tree-sitter taxonomy. `promoted_kinds` covers wrappers like `public_field_definition` or `variable_declarator` that count as a structure when their `value` field is a function body (JS/TS class arrow-fields, top-level `const f = () => {}`). `function_body_kinds` both gates promotion and demotes nested helpers (e.g. `fn inner` inside `fn outer`) so they don't steal the outer anchor.
- **Diff computation**: `deltoids::Diff::compute()` parses both old and new files, then runs two phases in `scope/`: `range.rs` plans `ContextRange`s per diff op (anchored on enclosing scope or default 3-line fallback), and `hunk_builder.rs` fills each range into a `Hunk` from the diff ops.
- **Hunk iteration**: Consumers walk hunks via `Hunk::runs()` -> `HunkRun` (`Header` / `Subhunk` / `Context`) instead of regrouping lines themselves. Both `render::render_hunk` and the TUI's `detail_items` share this iterator; reach for it before adding new line-grouping code.
- **TUI layout**: Three-pane lazygit-inspired layout (entries, traces, diff).
- **Traces**: Stored in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`.

## Diff cases (start here when changing the diff engine)

`crates/deltoids/tests/diff_cases/` is both an integration test and a
product reference for `deltoids::Diff::compute`. Each case directory
holds a description, an `original`/`updated` input pair, and an
`expected.diff` recording the engine's output. See
`tests/diff_cases/README.md` for the format.

**Whenever you change the diff engine** (`scope.rs`, `syntax.rs`,
`parse.rs`, `intraline.rs`, `reverse.rs`, hunk construction, breadcrumb
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
