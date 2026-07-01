# Agent Instructions

## Project Overview

This is a Rust workspace with CLI tools that trace file edits, plus a TUI to browse traces.

**Crates:**
- `deltoids` — diff library with tree-sitter scope context. Optional features:
  - `blob-resolve` — adds `git`/`content` modules for resolving before/after blob content from a git repo (used by the `pager` and `tui` subcommands).
  - `ratatui` — adds `render_tui` for rendering hunks/headers as `ratatui::text::Line<'static>` (used by the `tui` subcommand).
- `deltoids-cli` — ships a single `deltoids` binary with subcommands: `pager` (ANSI diff filter), `tui` (unified scrolling TUI: working-tree diff + trace browser), `edit`/`write` (agent edit tools). Also holds the trace-management library shared by `edit`/`write`. Cargo-dist publishes one homebrew formula (`deltoids`) and one shell installer for this crate.
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
    src/lib.rs               # Thin crate root: re-exports + shared helpers
    src/types.rs             # Wire request/response/error types
    src/edit.rs              # `edit` tool execution + apply_edits
    src/write.rs             # `write` tool execution
    src/hash_edit.rs         # `hashedit` execution + op translation
    src/hash_read.rs         # `hashread` rendering
    src/trace_store.rs       # Trace storage
    src/hashline/            # Hashline engine
      mod.rs                 #   docs + re-exports
      anchor.rs             #   hash alphabet, formatters, anchor parsing
      apply.rs              #   edit ops + splice engine
    src/sidebar/             # File tree sidebar for Files mode
      mod.rs                 #   Sidebar state + navigation
      status.rs            #   file classification
      tree.rs              #   path-tree construction
      icons.rs             #   nerd-font glyph tables
      render.rs            #   row -> styled line
      test_support.rs      #   shared test fixtures
    src/scroll.rs            # Mouse-wheel scroll feel
    src/cli.rs               # Subcommand module declarations
    src/cli/pager.rs         # `deltoids pager` subcommand
    src/cli/browse/          # unified scrolling TUI (files / traces)
      mod.rs                 #   mode-agnostic shell: loop, routing, layout,
                             #     divider, resize, wheel, mode cycling, help,
                             #     reload orchestration (active eager / lazy)
      mode.rs               #   Mode trait + TabStrip + AppCommand
      help.rs               #   shared help popup
      watch.rs              #   shared workdir watcher + reload filter
      tests.rs              #   shell tests (mock Mode)
      files/                 #   FilesMode (working-tree / piped diff)
        mod.rs               #     FilesMode impl of Mode
        model.rs             #     parse/resolve/diff
        diff_pane.rs         #     diff pane slice
        sidebar_pane.rs      #     sidebar pane slice
        reload.rs            #     working-tree watcher + rebuild
        test_support.rs      #     shared test fixtures
      traces/                #   TracesMode (edit/write trace browser)
        mod.rs               #     TracesMode impl of Mode
        model.rs             #     load traces/entries
        entries_pane.rs      #     entries list slice
        traces_pane.rs       #     traces list slice
        detail.rs            #     detail/diff slice (cache + renderers)
        reload.rs            #     reload from disk
        scripted.rs          #     headless render path
        test_support.rs      #     shared test fixtures
    src/cli/tui.rs           # `deltoids tui` entry (interactive / headless scripted)
    src/cli/edit.rs          # `deltoids edit` subcommand
    src/cli/write.rs         # `deltoids write` subcommand
    src/cli/hash_read.rs     # `deltoids hashread` subcommand
    src/cli/hash_edit.rs     # `deltoids hashedit` subcommand
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
- **One unified TUI, two modes.** `deltoids tui` (and bare `deltoids` on a TTY) opens the TUI (`cli/browse/`); the starting mode is smart (`browse::smart_initial_mode`): **Files** mode (`files/`) when the working tree has local changes, otherwise **Traces** mode (`traces/`). `]` cycles the left panel to the next mode and `[` to the previous, wrapping (Files → Traces → Files), lazygit-style; clicking a tab label in the top-left panel title switches straight to that mode (`TabStrip::hit_test` maps a click column to a mode index; `title_line` and `hit_test` share one `strip_pieces` layout). The shell stays generic over `[_; MODE_COUNT]` arrays (`MODE_COUNT == TAB_LABELS.len()`), so adding a mode is a localized change. (When stdout is not a TTY, `tui` falls back to the Traces headless scripted render, the `tui_cli.rs` contract.) The active mode's top-left panel title is a lazygit-style tab strip `─[1]─Files - Traces─` (`render_tui::pane_block_with_tabs`), with the active label bold-accent and the inactive one muted. The `Mode` seam (`mode.rs`) is a real seam: the shell (`mod.rs`) is mode-agnostic and owns the terminal, the event loop, the one draggable divider, the shared sidebar width, `<`/`>` resize, the help popup, mode switching (`[`/`]` cycling and tab clicks, both routed through `select_mode`), and reload orchestration; each mode owns its full vertical slice (state, keys, mouse, render, reload) behind the trait. Pane chrome still comes from `render_tui`; modes scroll by physical rows and hard-wrap long diff lines onto padded continuation rows. Live reload keeps the active mode fresh eagerly (after a debounce) and reloads the inactive mode lazily on its next activation. Every mode builds lazily, including the one shown at startup: the shell starts with empty placeholders, flips the active tab, draws a `Loading…` frame (`draw_loading`, the tab strip plus a centered message), then builds the mode on the next loop step. Neither launch nor a cycle blocks on a blank screen, and an inactive mode never pays its build/watcher cost (Files working-tree diff and recursive watcher, which scale with repo size) until first activated. The build is synchronous (the data crosses no thread), so input pauses during it, but the visible `Loading…` state shows why. Sidebar width is shell-level and shared across modes.
- **Shared workdir watcher is one file.** Files mode watches the repo working tree and filters the noise (`.git/` churn, gitignored paths). That essence lives once in `browse/watch.rs` (`spawn_workdir_watcher`, `path_warrants_reload`, `is_git_internal`); the mode delegates rather than inlining it. Files mode's `DiffSource::WorkingTree` arm calls it; Files' `Static` arm keeps returning "no watcher / never reload".
- **Big files become directories.** A source file past ~1,000 lines with separable concerns becomes a `foo/` directory split by change axis (the pane/feature that changes together, not by layer): `mod.rs` holds the entry point and glue, one file per named concern owns its full vertical slice, tests live beside their code, and shared fixtures go in one `test_support.rs`. `scope/` is the original precedent; `cli/browse/` (with its `files/` and `traces/` mode subtrees), `sidebar/`, and `hashline/` follow it. The crate root (`lib.rs`) cannot be a directory, so it carves concerns into sibling modules (`edit.rs`, `write.rs`, `hash_edit.rs`, `hash_read.rs`, `types.rs`) and stays a thin re-export root.
- **Scroll feel is one file.** All wheel-burst smoothing lives in `scroll.rs` (`WheelScroll`); both TUIs route wheel events through it, so changing scroll feel is a one-file edit they both inherit.
- **Sidebar sizing is one file.** All clamp/fraction/step/divider math lives in `sidebar_width.rs`; change sizing policy there. Sidebars never hide.
- **Default behavior.** Plain `deltoids` runs `pager` when stdin is piped (preserving `git config core.pager 'deltoids | less -R'`) and opens the `tui` on a TTY. The TUI no longer reads a piped diff; piped diffs always go to the pager.
- **Trace storage.** Traces live in `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`; `hashedit` records as `EditHistoryEntry { tool: "hashedit" }` with synthesised per-op `TextEdit`s (preserving each `reason`) so the trace TUI and pager work unchanged.
- **Edit modes.** `DELTOIDS_EDIT_MODE` (`text` default, `hash`) is read once at extension load to keep the system prompt static for prompt caching; switching modes needs a pi restart. Both modes override pi's `edit`/`write`; hash mode also adds `hashread` and re-describes `read` to steer text reads toward it (`read`'s real implementation still handles images, dirs, URLs, archives, etc.). The hash is a validation token, not an address. Engine: `hashline/`.

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
