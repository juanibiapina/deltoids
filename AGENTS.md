# Agent Instructions

## Project Overview

This is a Rust workspace for a diff toolkit: an ANSI diff pager, a scrolling TUI over the working-tree diff, and a browser PR reviewer.

**Crates:**
- `deltoids` — diff library with tree-sitter scope context. Optional features:
  - `blob-resolve` — adds `git`/`content` modules for resolving before/after blob content from a git repo (used by the `pager` and `tui` subcommands).
  - `ratatui` — adds `render_tui` for rendering hunks/headers as `ratatui::text::Line<'static>` (used by the `tui` subcommand).
  - `html` — adds `render_html` for rendering hunks as semantic HTML (used by the wasm reviewer).
  - `lang-<name>` — one per tree-sitter grammar, all on by default. Without one, that language's files diff like an unknown language: no breadcrumbs, hunk expansion, definitions, or symbols. `tests/grammar_off.rs` checks this with `--no-default-features`.
- `deltoids-cli` — ships a single `deltoids` binary with subcommands: `pager` (ANSI diff filter), `tui` (scrolling TUI over the working-tree diff). Cargo-dist publishes one homebrew formula (`deltoids`) and one shell installer for this crate.
- `deltoids-wasm` — WebAssembly build of the diff engine for the browser PR reviewer at `review.deltoids.dev` (the React app in `reviewer/`). A `cdylib` exposing `render_file`/`render_from_patch` over a C-ABI; builds for `wasm32-wasip1` via wasi-sdk. See `crates/deltoids-wasm/AGENTS.md`.
- `tests` — cross-crate integration tests

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
cargo deny check bans licenses sources
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
    src/render_tui.rs         # Diff rendering for ratatui (+ line_cells/byte_at: source bytes <-> wrapped cells)
    src/render_html.rs        # Diff rendering as HTML (feature `html`)
    src/git.rs                # Git blob lookup
    src/git/history.rs        # Local branches, commit walks, commit patches
    src/content.rs            # Before/after content resolution
    src/intraline.rs          # Within-line diff algorithm
    src/reverse.rs            # Diff reversal
    src/language.rs           # Language detection and config
    src/syntax.rs             # Tree-sitter parsing, scopes, and scope names
    src/syntax/outline.rs     # A parsed file's definitions, with hashes of each one's text
    src/syntax/visibility.rs  # Who can use a definition: per-language rules, capped by its container
    src/definitions.rs        # Definition changes between two versions (DefinitionDiff)
    src/definitions/matching.rs # Pair old and new definitions: by name, rename, move
    src/definitions/classify.rs # What changed in each pair, as a tree in file order
    src/symbols.rs            # Symbols: identifiers on a line (what go-to-definition asks about)
    tests/diff_cases.rs       # Diff-case suite + exact-cover random-edit sweep
    tests/diff_cases/         # Diff-case harness, exact-cover check, and cases
    tests/diff_compute.rs     # Diff::compute property tests
    tests/definitions.rs      # DefinitionDiff reports per language and kind of change
      cases/<NNN-slug>/       # One case per directory

  deltoids-cli/
    src/lib.rs               # Thin crate root: module declarations
    src/sidebar/             # File tree sidebar
      mod.rs                 #   Sidebar state + navigation
      status.rs            #   file classification
      tree.rs              #   path-tree construction (hidden files left out)
      icons.rs             #   nerd-font glyph tables
      render.rs            #   row -> styled line
      test_support.rs      #   shared test fixtures
    src/judgments/           # Jev judgments: role + attention per hunk
      mod.rs                 #   change model, absorb Jev answers, file notes
      request.rs             #   Jev request bodies and batching
      paths.rs               #   files Jev never sees (lockfiles, generated)
      jev.rs                 #   blocking HTTP transport + TYPESAFE_API_KEY
      tests.rs               #   interface tests; fixtures/ holds recorded
                             #     patches and Jev answers (re-record with
                             #     `record_jev_fixtures -- --ignored`)
    src/lsp/                 # Language servers: definition, callers, and hover info for a symbol
      mod.rs                 #   Lsp handle (warm/ask/answer/cancel/progress/sync/poll)
      server.rs              #   one server: handshake, queue, documents, requests, progress
      callers.rs             #   callers lookup: every use of any symbol (call
                             #     hierarchy + references, calls through a trait)
      watched_files.rs       #   files servers watch: their registrations, and the
                             #     changes the workdir watcher feeds (Lsp::file_events)
      transport.rs           #   child-process adapter (stdio framing via lsp-server)
      config.rs              #   [lsp.<language>] tables over built-in defaults
                             #     (commands, rust-analyzer settings)
      position.rs            #   file URIs, UTF-8/UTF-16 columns
      fake.rs, tests.rs      #   scripted in-memory server + interface tests (one
                             #     ignored test against real rust-analyzer;
                             #     RUST_ANALYZER=<path> to pick the binary)
    src/cli.rs               # Subcommand module declarations
    src/cli/pager.rs         # `deltoids pager` subcommand
    src/cli/browse/          # scrolling TUI
      mod.rs                 #   shell: loop, routing, layout, divider,
                             #     resize, help, reload orchestration
      mode.rs               #   Mode trait (seam to Workspace) + AppCommand
      events.rs             #   input-event burst draining
      terminal.rs           #   terminal session guard
      sidebar_width.rs      #   sidebar sizing policy
      scroll.rs             #   mouse-wheel scroll feel
      help.rs               #   help popup
      theme_picker.rs       #   live syntax-theme picker popup (`t`)
      syntax_badge.rs       #   file-header language / scope-support badge
      comments.rs           #   review comments
      comment_view.rs       #   comment rows
      diff_cursor.rs        #   the cursor that walks lines
      clipboard.rs          #   clipboard write (native helper + OSC 52)
      text.rs               #   display width + word wrapping
      watch.rs              #   shared workdir watcher + reload filter;
                            #     also feeds the language servers' changes
      tests.rs              #   shell tests (mock Mode)
      test_screen.rs        #   draw any Mode to text (test helper)
      panel.rs              #   Panel trait (side panel + main view) + titles
      workspace/             #   root Mode: lazygit layout, focus, routing
        mod.rs               #     panel stack, keys, mouse, reload
        tests.rs             #     layout and navigation tests
      history/               #   [2] Branches panel (local, or GitButler's)
        mod.rs               #     drill-down to a branch's commits, list rendering
        cursor.rs            #     list selection + scroll
        tree.rs              #     branch rows; GitButler workspace drawn like `but status`
      files/                 #   FilesMode (working-tree diff; read-only snapshots)
        snapshot.rs          #     commit or branch snapshot panel, built off the UI thread
        mod.rs               #     FilesMode impl of Mode
        model.rs             #     parse/resolve/diff
        diff_pane.rs         #     diff pane slice
        overview.rs          #     summary of a directory or commit: file headers + blocks
        definitions_view.rs  #     one file's definition changes, alone or among unchanged
                             #       ones as context (or hunks), drawn in the background
        definitions_tests.rs #     definitions view and summaries through keys and mouse
        stage_panes.rs       #     which staging column the diff shows, with which files
        sidebar_pane.rs      #     sidebar pane slice
        reload.rs            #     working-tree watcher + rebuild
        review.rs            #     review guidance: Jev job, judgments, row notes
        navigate.rs          #     go to definition/callers, jumps, Ctrl+o, visited files
        hover/               #     hover popup: pointer rest or `i`, server info by the symbol
          mod.rs             #       wait, request, place, scroll, close rules
          markdown.rs        #       hover Markdown -> wrapped, highlighted lines
        navigation_tests.rs  #     navigation and hover through keys, mouse, and screen (fake Lsp)
        test_support.rs      #     shared test fixtures
      vcs/                   #   git vs GitButler: the only place they differ
        mod.rs               #     Vcs (reads) + Writer (stage/discard)
        git_writes.rs        #     git adapter: index + git CLI writes
        gitbutler.rs         #     GitButler adapter: `but --json`
        gitbutler/tests.rs   #     GitButler tests (stub `but`)
        tests.rs             #     git writer tests
        fixtures/            #     recorded `but --json` output
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
deployed to `review.deltoids.dev` as a Cloudflare Worker (static
assets plus a small `/api` backend with Clerk sign-in) by the
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
