# Agent Instructions

## Project Overview

This is a Rust workspace for a diff toolkit: an ANSI diff pager, a scrolling TUI over the working-tree diff, and a browser PR reviewer.

**Crates:**
- `deltoids` — diff library with tree-sitter scope context. Optional features:
  - `blob-resolve` — adds `git`/`content` modules for resolving before/after blob content from a git repo (used by the `pager` and `tui` subcommands).
  - `ratatui` — adds `render_tui` for rendering hunks/headers as `ratatui::text::Line<'static>` (used by the `tui` subcommand).
  - `html` — adds `render_html` for rendering hunks as semantic HTML (used by the wasm reviewer).
- `deltoids-cli` — ships a single `deltoids` binary with subcommands: `pager` (ANSI diff filter), `tui` (scrolling TUI over the working-tree diff). Cargo-dist publishes one homebrew formula (`deltoids`) and one shell installer for this crate.
- `deltoids-wasm` — WebAssembly build of the diff engine for the browser PR reviewer at `review.deltoids.dev` (the React app in `reviewer/`). A `cdylib` exposing `render_file`/`render_from_patch` over a C-ABI; builds for `wasm32-wasip1` via wasi-sdk. See `crates/deltoids-wasm/AGENTS.md`.
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
    build.rs                  # Converts vendored themes to syntect dumps
    assets/themes/            # Vendored .tmTheme sources (e.g. Tokyo Night)
    src/lib.rs                # Library exports
    src/config.rs             # Theme + syntax-theme registry (theme_by_name/theme_names)
    src/engine.rs             # Line-level diff engine
    src/parse.rs              # Git diff parsing
    src/scope.rs              # Hunk types and Diff::compute
    src/scope/expansion.rs    # Context window per change (hunk expansion rules)
    src/scope/hunks.rs        # Grouping, emission, breadcrumbs (exact cover)
    src/hunk_header.rs        # Shared header layout
    src/render.rs             # Diff rendering as ANSI
    src/render_tui.rs         # Diff rendering for ratatui
    src/render_html.rs        # Diff rendering as HTML (feature `html`)
    src/git.rs                # Git blob lookup
    src/content.rs            # Before/after content resolution
    src/intraline.rs          # Within-line diff algorithm
    src/reverse.rs            # Diff reversal
    src/language.rs           # Language detection and config
    src/syntax.rs             # Tree-sitter parsing and scopes
    tests/diff_cases.rs       # Diff-case suite + exact-cover random-edit sweep
    tests/diff_cases/         # Diff-case harness, exact-cover check, and cases
    tests/diff_compute.rs     # Diff::compute property tests
      cases/<NNN-slug>/       # One case per directory

  deltoids-cli/
    src/lib.rs               # Thin crate root: module declarations
    src/sidebar/             # File tree sidebar
      mod.rs                 #   Sidebar state + navigation
      status.rs            #   file classification
      tree.rs              #   path-tree construction
      icons.rs             #   nerd-font glyph tables
      render.rs            #   row -> styled line
      test_support.rs      #   shared test fixtures
    src/scroll.rs            # Mouse-wheel scroll feel
    src/cli.rs               # Subcommand module declarations
    src/cli/pager.rs         # `deltoids pager` subcommand
    src/cli/browse/          # scrolling TUI
      mod.rs                 #   shell: loop, routing, layout, divider,
                             #     resize, help, reload orchestration
      mode.rs               #   Mode trait (seam to FilesMode) + AppCommand
      help.rs               #   help popup
      theme_picker.rs       #   live syntax-theme picker popup (`t`)
      syntax_badge.rs       #   file-header language / scope-support badge
      comments.rs           #   review comments
      comment_view.rs       #   comment rows
      diff_cursor.rs        #   the cursor that walks lines
      clipboard.rs          #   clipboard write (native helper + OSC 52)
      text.rs               #   display width + word wrapping
      watch.rs              #   shared workdir watcher + reload filter
      tests.rs              #   shell tests (mock Mode)
      files/                 #   FilesMode (working-tree / piped diff)
        mod.rs               #     FilesMode impl of Mode
        model.rs             #     parse/resolve/diff
        diff_pane.rs         #     diff pane slice
        stage_panes.rs       #     which staging column the diff shows, with which files
        sidebar_pane.rs      #     sidebar pane slice
        reload.rs            #     working-tree watcher + rebuild
        test_support.rs      #     shared test fixtures
    src/cli/tui.rs           # `deltoids tui` entry (requires a terminal)
    src/bin/deltoids.rs      # Single binary dispatcher

  deltoids-wasm/
    src/lib.rs               # cdylib: alloc/dealloc + render_file/render_from_patch
    build-wasm.sh            # wasi-sdk build + wasm-opt -> reviewer/public/ (DEST overridable)
    AGENTS.md                # wasm build, feature setup, and web app notes

  tests/
    tests/cli_surface.rs      # Integration tests for the public CLI surface
    tests/pager_*.rs          # Integration tests for the pager

reviewer/         # standalone React PR reviewer at review.deltoids.dev (see reviewer/AGENTS.md)
  src/core/       # framework-neutral core (engine, github, lib)
  src/components/ # React UI (Topbar, Sidebar, FileCard, ...)
  src/hooks/      # prefs + topbar-height hooks
```

## Site

Marketing/landing site for `deltoids.dev`, under `site/` (bare Astro,
no integrations, minimal client JS). Deploys to GitHub Pages from the
`Pages` workflow on push to `main` when `site/**` changes. Self-hosted
IBM Plex Sans + JetBrains Mono.

Local dev (from `site/`): `npm install`, `npm run dev` (`:4321`),
`npm run build`, `npm run preview`, `npx astro check`.

See `site/AGENTS.md` for component conventions and the release
checklist.

## Reviewer

The browser PR reviewer is the standalone React app in `reviewer/`,
deployed to `review.deltoids.dev` on Cloudflare Pages by the
`Reviewer` workflow (`.github/workflows/reviewer.yml`) on `reviewer/**`
or wasm-crate changes. Its wasm engine (`crates/deltoids-wasm`) is
built (wasi-sdk + wasm-opt) into `reviewer/public/deltoids_wasm.wasm`
before the Vite build. See `reviewer/AGENTS.md`.

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

All workspace crates track the same version. To prep a release, bump the version in **every** file below
in a single `release: X.Y.Z` commit:

- `Cargo.toml` (`workspace.package.version`)
- `Cargo.lock` (run `cargo update -p deltoids -p deltoids-cli -p tests` after editing `Cargo.toml`)
- `site/src/data/site.ts` (`SITE.version`)
- `CHANGELOG.md` (cut a new dated section under `[Unreleased]`)

Then push `main` and push a `vX.Y.Z` tag. The `release.yml` workflow
is triggered by the tag and runs cargo-dist, which builds the shell
installer and macOS/Linux archives and publishes the homebrew formula
to `juanibiapina/homebrew-taps`.

## Conventions

- Run `cargo fmt --all` before committing.
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
- For diff-engine changes, the diff-case suite is the canonical test
  surface (see above). Every case and a seeded random-edit sweep must
  also show each changed line exactly once in ordered, disjoint hunks
  (`tests/diff_cases/exact_cover.rs`). Narrow `Diff::compute`
  properties live in `tests/diff_compute.rs`.
