# Agent Instructions

## Project Overview

This is a Rust workspace for a diff toolkit: an ANSI diff pager, a scrolling TUI over the working-tree diff, and a browser PR reviewer.

- `crates/deltoids`: the diff library, with tree-sitter scope context. Optional features:
  - `blob-resolve`: `git`/`content` modules that resolve before/after blob content from a git repo (used by `pager` and `tui`).
  - `ratatui`: `render_tui`, which renders hunks and headers as ratatui lines (used by `tui`).
  - `html`: `render_html`, which renders hunks as semantic HTML (used by the wasm engine).
  - `lang-<name>`: one per tree-sitter grammar, all on by default. Without one, files in that language diff like an unknown language: no breadcrumbs, hunk expansion, definitions, or symbols.
- `crates/deltoids-cli`: the single `deltoids` binary, with the subcommands `pager` (ANSI diff filter) and `tui` (scrolling TUI). Cargo-dist publishes one homebrew formula and one shell installer for this crate.
- `crates/deltoids-wasm`: the WebAssembly build of the engine for `reviewer/`. Before you change files under `crates/deltoids-wasm/`, read `crates/deltoids-wasm/AGENTS.md`.
- `crates/tests`: cross-crate integration tests (CLI surface, pager).
- `reviewer/`: the browser PR reviewer at `review.deltoids.dev`. Before you change files under `reviewer/`, read `reviewer/AGENTS.md`.
- `site/`: the marketing site at `deltoids.dev`. Before you change files under `site/`, read `site/AGENTS.md`.

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
cargo deny check bans licenses sources
cargo install --path crates/deltoids-cli  # installs the `deltoids` binary
```

## Where things are

- `crates/deltoids/tests/diff_cases/`: the diff-case suite (see Diff cases).
- `crates/deltoids/tests/grammar_off.rs`: checks the `lang-<name>` fallback with `--no-default-features`.
- `crates/deltoids-cli/src/judgments/fixtures/`: recorded patches and Jev answers. Re-record them with `cargo test -p deltoids-cli --lib record_jev_fixtures -- --ignored`.
- `crates/deltoids-cli/src/sidebar/tree.rs`: canonical for `reviewer/src/core/filetree.ts`. Port changes.
- `crates/deltoids-cli/src/cli/browse/scroll.rs`: canonical for `reviewer/src/core/wheel.ts`. Port changes.
- `crates/deltoids-cli/src/cli/browse/diff_scrollbar.rs`: canonical for `reviewer/src/core/overview.ts`. Port changes.

## Diff cases (start here when changing the diff engine)

`crates/deltoids/tests/diff_cases/` is both an integration test and a
product reference for `deltoids::Diff::compute`. Each case directory
holds a description, an `original`/`updated` input pair, and an
`expected.diff` recording the engine's output. See
`tests/diff_cases/README.md` for the format.

Whenever you change the diff engine (`scope.rs`, `syntax.rs`,
`language.rs`, `parse.rs`, `intraline.rs`, `reverse.rs`, hunk construction,
breadcrumb rules), follow this loop:

1. Pick the case that matches the behavior, or add a new one. New cases
   start with `1-case.md` (the explainer), `2-original.<EXT>`, and
   `3-updated.<EXT>`. Keep the inputs minimal.
2. Run the suite to see the impact:
   ```bash
   cargo test -p deltoids --test diff_cases
   ```
   The failure output diffs recorded against actual for every moved case.
3. When the new behavior is what you want, refresh expectations:
   ```bash
   DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test diff_cases
   ```
   Inspect every changed `4-expected.diff` by hand: these files are the spec.
4. If a case moved in a way you did not intend, fix the implementation,
   not the expected output.

For new behavior, add the case before the implementation: the failing
case becomes the spec.

## Releasing

All workspace crates track the same version. In a single `release: X.Y.Z`
commit:

1. Bump `workspace.package.version` in `Cargo.toml`.
2. Run `cargo update -p deltoids -p deltoids-cli -p tests` to update `Cargo.lock`.
3. Bump `SITE.version` in `site/src/data/site.ts`. The site deploys on push
   to `main`, so it shows the new version when the release goes out.
4. Cut a new dated section under `[Unreleased]` in `CHANGELOG.md`.
5. Compare `INSTALL_CARDS`, `FAQ`, and `REVIEW_FEATURES` in
   `site/src/data/site.ts` and the pages under `site/src/pages/docs/` with
   the new CHANGELOG section, and update them.

Then push `main` and push a `vX.Y.Z` tag. The tag triggers `release.yml`,
which runs cargo-dist: it builds the shell installer and the macOS/Linux
archives and publishes the homebrew formula to `juanibiapina/homebrew-taps`.

## Conventions

- Run `cargo fmt --all` before you commit.
- Extract small, single-purpose helpers over generic utility modules.
- Add tests alongside refactors.
- For diff-engine changes, the diff-case suite is the main test surface.
  Every case and a seeded random-edit sweep must also show each changed
  line exactly once in ordered, disjoint hunks
  (`tests/diff_cases/exact_cover.rs`). Narrow `Diff::compute` properties
  live in `tests/diff_compute.rs`.
