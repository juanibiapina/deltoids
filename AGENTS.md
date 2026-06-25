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
    src/lib.rs                # Library exports
    src/engine.rs             # Line-level diff engine
    src/parse.rs              # Git diff parsing
    src/scope.rs              # Hunk types and scope context
    src/scope/range.rs        # Context-range planning phase
    src/scope/hunk_builder.rs # Hunk filling phase
    src/hunk_header.rs        # Shared header layout
    src/render.rs             # Diff rendering as ANSI
    src/render_tui.rs         # Diff rendering for ratatui
    src/git.rs                # Git blob lookup
    src/content.rs            # Before/after content resolution
    src/intraline.rs          # Within-line diff algorithm
    src/reverse.rs            # Diff reversal
    src/language.rs           # Language detection and config
    src/syntax.rs             # Tree-sitter parsing and scopes
    tests/diff_cases.rs       # Diff-case suite entry point
    tests/diff_cases/         # Diff-case harness and cases
      cases/<NNN-slug>/       # One case per directory

  deltoids-cli/
    src/lib.rs               # Library entry: edit/write execution
    src/trace_store.rs       # Trace storage
    src/hashline.rs          # Hashline engine
    src/tui.rs               # `traces` subcommand chrome
    src/sidebar.rs           # File tree sidebar for `review`
    src/scroll.rs            # Mouse-wheel scroll feel
    src/cli.rs               # Subcommand module declarations
    src/cli/pager.rs         # `deltoids pager` subcommand
    src/cli/review.rs        # `deltoids review` scrolling diff TUI
    src/cli/edit.rs          # `deltoids edit` subcommand
    src/cli/write.rs         # `deltoids write` subcommand
    src/cli/hash_read.rs     # `deltoids hashread` subcommand
    src/cli/hash_edit.rs     # `deltoids hashedit` subcommand
    src/cli/traces.rs        # `deltoids traces` subcommand
    src/cli/hook.rs          # `deltoids hook` subcommand
    src/bin/deltoids.rs      # Single binary dispatcher

  tests/
    tests/tui_cli.rs          # Integration tests for edit/write/traces
    tests/hash_cli.rs         # Integration tests for hashread/hashedit
    tests/claude_code_hook.rs # Integration tests for the hook
    fixtures/claude-code/     # Hook test fixtures

plugins/
  pi/             # Pi extension
  claude-code/    # Claude Code plugin

.claude-plugin/
  marketplace.json # Claude Code plugin marketplace
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

Each entry names the module that owns a concern and the invariant to respect. Read that module for the implementation detail (algorithms, kind tables, helpers).

- **Tree-sitter scope context.** Diffs expand each change to its enclosing context. Model: the tree-sitter ancestor chain is the hierarchy; *structures* (functions, methods, classes, impls, modules) are boundaries that both cap upward growth and form the breadcrumb; every other ancestor is transparent and absorbed by expansion. `ParsedFile` (`syntax.rs`) is the only seam onto tree-sitter — callers never touch raw node kinds; `language.rs` owns detection and the per-language kind tables. Keep two questions separate: how far to expand a contained change (`expansion_anchor`) vs whether a new structure deserves its own hunk (`scope_for_range`/`scope_at`). Kind tiers, promotion, and per-language gating (e.g. Markdown) live in those modules.
- **Diff pipeline.** `Diff::compute()` runs the line-level engine (`engine.rs`), detects one stable `Language`, then two phases in `scope/`: range planning (`range.rs`) and hunk building (`hunk_builder.rs`).
- **Hunk iteration.** Walk hunks via `Hunk::runs()` -> `HunkRun` (`Header`/`Subhunk`/`Context`), not by regrouping lines. `render_hunk` and the TUI's `detail_items` share it; reach for it before adding new line-grouping code.
- **Per-line stateless highlighting.** Each diff line is highlighted with fresh syntect state, so stateful grammars lose context-dependent color (e.g. a Dockerfile `RUN` without its `FROM`). Known limitation, not a goal.
- **TUI chrome is shared.** `review` and `traces` share pane chrome from `render_tui`; both scroll by physical rows, and the diff body hard-wraps long lines onto padded continuation rows.
- **Scroll feel is one file.** All wheel-burst smoothing lives in `scroll.rs` (`WheelScroll`); both TUIs route wheel events through it, so changing scroll feel is a one-file edit they both inherit.
- **Sidebar sizing is one file.** All clamp/fraction/step/divider math lives in `sidebar_width.rs`; change sizing policy there. Sidebars never hide.
- **Default behavior.** Plain `deltoids` runs `pager` when stdin is piped (preserving `git config core.pager 'deltoids | less -R'`) and prints help on a TTY.
- **Trace storage.** Traces live in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`; `hashedit` records as `EditHistoryEntry { tool: "hashedit" }` with synthesised per-op `TextEdit`s (preserving each `reason`) so the trace TUI and pager work unchanged.
- **Edit modes.** `DELTOIDS_EDIT_MODE` (`text` default, `hash`) is read once at extension load to keep the system prompt static for prompt caching; switching modes needs a pi restart. Both modes override pi's `edit`/`write`; hash mode also adds `hashread` and re-describes `read` to steer text reads toward it (`read`'s real implementation still handles images, dirs, URLs, archives, etc.). The hash is a validation token, not an address. Engine: `hashline.rs`.

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

## Releasing

All workspace crates and the Claude Code plugin track the same
version. To prep a release, bump the version in **every** file below
in a single `release: X.Y.Z` commit:

- `Cargo.toml` (`workspace.package.version`)
- `Cargo.lock` (run `cargo update -p deltoids -p deltoids-cli -p tests` after editing `Cargo.toml`)
- `site/src/data/site.ts` (`SITE.version`)
- `plugins/claude-code/.claude-plugin/plugin.json` (`version`)
- `.claude-plugin/marketplace.json` (`version`)
- `CHANGELOG.md` (cut a new dated section under `[Unreleased]`)

Then push `main` and push a `vX.Y.Z` tag. The `release.yml` workflow
is triggered by the tag and runs cargo-dist, which builds the shell
installer and macOS/Linux archives and publishes the homebrew formula
to `juanibiapina/homebrew-taps`. The Claude Code plugin marketplace is
served straight from `main`, so the plugin bump must land before any
user re-runs `claude plugin install`.

## Conventions

- Run `cargo fmt --all` before committing.
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
- For diff-engine changes, the diff-case suite is the canonical test
  surface (see above). Inline `#[test]`s in `scope.rs` etc. remain
  useful for narrow property assertions; cases are the broad spec.
