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

- **Tree-sitter scope context (one chain, boundaries, transparent expansion)**: Diffs expand hunks to show enclosing context. The model is: the tree-sitter ancestor chain from a change to the file root is the hierarchy; **boundaries** (named structures: functions, methods, classes, impls, modules, plus the promoted / labeled-callback / anchor-only refinements) both stop upward growth (the ceiling) and form the breadcrumb; every other ancestor (assignment, call, argument list, literal) is **transparent** and absorbed by expansion automatically, never enumerated. Configuration in `deltoids/src/scope.rs` (`MAX_SCOPE_LINES = 200`). `deltoids/src/language.rs` owns stable language detection (bundled syntect path/shebang detection), `Language` ids, tree-sitter parser selection, and per-language node-kind tables. The public parsing surface is `ParsedFile` in `deltoids/src/syntax.rs`, which owns the parsed source and exposes `enclosing_scopes(line)` (the filtered breadcrumb chain, structures only), `is_structure(scope)`, and `expansion_anchor(start, end, budget)` (the kind-agnostic outward walk over the raw chain, used when no enclosing structure exists). Callers never touch raw tree-sitter taxonomy. Two questions are kept separate: **expansion** ("how far do I grow context for a contained change?") is `expansion_anchor`, used only in the contained-change fallbacks of `range.rs`; **new-scope detection** ("is there a brand-new structure that deserves its own hunk?") is structure-only via `scope_for_range`/`scope_at`. `promoted_kinds` covers wrappers like `public_field_definition` or `variable_declarator` that count as a structure when their `value` field is a function body (JS/TS class arrow-fields, top-level `const f = () => {}`). `function_body_kinds` both gates promotion and demotes nested helpers (e.g. `fn inner` inside `fn outer`) so they don't steal the outer anchor. (The old `data_kinds` tier has been retired: anonymous containers are just transparent ancestors now.) Transparent expansion is gated per language by `transparent_expansion` in `TreeSitterConfig`; it is **off for Markdown** (prose), where the outermost transparent ancestor is a whole heading-delimited `section` that would swallow the file, so Markdown body changes use 3-line default context until a section-aware model lands.
- **Diff computation**: `deltoids::Diff::compute()` first runs `engine::Snapshot::compute()` (line-level diff via `gix-imara-diff` Histogram + line postprocessing) to produce a `Vec<DiffOp>` and unified text. It then detects one stable `Language` from the path plus in-memory snapshots, parses both old and new snapshots with that language, and runs two phases in `scope/`: `range.rs` plans `ContextRange`s per diff op (anchored on enclosing scope or default 3-line fallback), and `hunk_builder.rs` fills each range into a `Hunk` from the diff ops. `engine::align_old_to_new(line, ops)` is the shared helper for mapping OLD line numbers through the diff (used by `same_slot` rename detection).
- **Hunk iteration**: Consumers walk hunks via `Hunk::runs()` -> `HunkRun` (`Header` / `Subhunk` / `Context`) instead of regrouping lines themselves. Both `render::render_hunk` and the TUI's `detail_items` share this iterator; reach for it before adding new line-grouping code.
- **Highlighting is per-line and stateless**: `render::highlight_line` (and `render_tui::highlighted_spans`) highlight each diff line with fresh syntect state. Context-free tokens color fine, but stateful grammars lose context-dependent coloring (e.g. a Dockerfile `RUN`/`ENV` keyword stays uncolored without the preceding `FROM`). Fixing it needs state carried across hunk lines from the file top.
- **TUI layout**: Three-pane lazygit-inspired layout (entries, traces, diff). Both `deltoids review` and `deltoids traces` use shared pane chrome from `deltoids::render_tui`. Diff body lines longer than the pane width wrap onto continuation rows (hard char wrap) instead of being cut; each wrapped row is padded to the width with the line's diff background. Both TUIs scroll by physical rows, so the extra rows need no scroll-math changes. The breadcrumb box still truncates ancestor text to its fixed geometry. The shared renderer feeds a single char-production path into a `CharSink` (`render_tui.rs`): `TruncateSink` (breadcrumb, single capped row) or `WrapSink` (diff body, padded wrapped rows). The ANSI pager (`render.rs`) is unchanged and relies on `less -R` to soft-wrap.
- **Mouse-wheel scroll feel**: One physical wheel tick fans out into a burst of events; applying one motion per event makes lists jump. `crate::scroll::WheelScroll<K>` (in `deltoids-cli/src/scroll.rs`) counts the burst and returns how many steps to apply, keyed by pane and direction (primed so the first event of a gesture moves at once). The quotas live there (`ScrollKind::List` = stepped/slow, `ScrollKind::Content` = smooth one-line). Both TUIs map their wheel events onto `WheelScroll::advance` and apply the returned step count to that pane's own motion, so changing scroll feel is a one-file edit that every TUI inherits.
- **Sidebar sizing**: The min/max clamp, the terminal fraction, the resize step, and the divider-drag math live only in `deltoids-cli/src/sidebar_width.rs`. Both `review` and `traces` own a `Preference` (seeded from terminal width) and call `effective`/`widen`/`narrow`/`set_from_divider`/`diff_pane_width`; the constants are private to the module. Neither sidebar ever hides — `effective` clamps to at least the minimum width on any terminal. Change sizing policy in that one file.
- **Default behavior**: Plain `deltoids` (no subcommand) runs `pager` when stdin is piped (preserving `git config core.pager 'deltoids | less -R'`) and prints help when stdin is a TTY.
- **Traces**: Stored in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`. `hashedit` records as `EditHistoryEntry { tool: "hashedit" }` with synthesised `TextEdit`s (one per op, preserving per-op `reason`) so the existing trace TUI and pager work unchanged.
- **Hashline edit mode**: Selected once at extension load by `DELTOIDS_EDIT_MODE=hash` (defaults to `text`); changing modes requires restarting pi. This keeps the system prompt static so pi's prompt caching stays warm. In both modes the extension overrides pi's built-in `edit` and `write`; the mode only changes which `deltoids` subcommand `edit` spawns (`edit` vs `hashedit`) and its schema. Hash mode additionally registers `hashread` for text-file reads with anchors, and **overrides pi's built-in `read` description** (execute path unchanged) to steer the model away from `read` for text files — reading text with `read` would force a re-read through `hashread` before editing, wasting a turn. `read`'s real implementation still handles images, dirs, URLs, archives, SQLite, etc. The hash is a validation token — the model addresses lines by number; the hash detects "the file changed since you last read it" at apply time. Engine: `crates/deltoids-cli/src/hashline.rs`.

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
