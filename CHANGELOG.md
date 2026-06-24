# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- diff: Syntax highlighting now works for any bundled syntect syntax (e.g. Dockerfile), independent of tree-sitter scope support.

### Changed

- TUI: Mouse-wheel scrolling is smoother throughout. Diff views and lists (the file sidebar, traces, and entries) now move in proportion to how much you scroll, instead of jumping too far per gesture.

## [0.9.0] - 2026-06-23

### Added

- review TUI: The sidebar is now resizable. Use `<`/`>` to narrow/widen it, or drag the divider between the panes with the mouse.

### Changed

- traces TUI: Entries pane now shows the filename first with a dimmed reason, instead of reason only.
- diff: Reworked how hunks grow their surrounding context, fixing a range of edge cases where edits inside wrapped statements, calls, and literals showed too little (or too much) context.

### Fixed

- tui: fix freeze with blank screen at 100% on macOS.
- diff: Wrapping a function body in a new call expression (e.g. `withDORetry(() => { ... })`) no longer duplicates the added lines in a ghost second hunk.

## [0.8.1] - 2026-05-27

### Fixed

- pi plugin: Steer model to use `read` (not `hashread`) for skill files in hash mode.
- diff: Replacing a function body with code containing nested callbacks (arrow functions, call expressions) no longer drops the added lines from the hunk.

## [0.8.0] - 2026-05-22

### Added

- tui: Mouse support in `deltoids traces` and `deltoids review`

### Fixed

- diff: Deleting lines inside a class-level decorator (e.g. `@Module({...})`) no longer produces an empty diff.
- diff: Method renames no longer split across two hunks when a new wrapper method is added nearby.
- diff: Scope expansion for a function body edit now includes leading doc comments and attributes (`///`, `#[inline]`, etc.) in the hunk context.

## [0.7.0] - 2026-05-18

### Added

- cli: `deltoids -v` / `deltoids --version` print the version.
- Add support for [hashedit](https://blog.can.ac/2026/02/12/the-harness-problem/) (line+hash anchored edits). Enable in the pi extension by setting `DELTOIDS_EDIT_MODE=hash`.
- pi: Add custom `renderCall`/`renderResult` for the `hashread` tool so it shows a collapsible box (first 10 lines, expandable) matching Pi's built-in tool chrome.

### Changed

- diff: Large functions no longer cliff from full context to 3-line context at 200 lines. Changes within 100 lines of each other merge into a single hunk, with up to 100 lines of context before and after. A 500-line function with changes spread every 80 lines now shows the entire function in one hunk instead of fragmenting into tiny disconnected pieces.
- tui: Pane titles in `deltoids review` and `deltoids traces` now use lazygit-style dash padding (e.g. `─[1]─Entries─`) so the title blends into the rounded border instead of leaving gaps.
- pi: Clear inherited renderers (`renderCall`, `renderResult`, `renderShell`) from `edit`, `write`, and `read` overrides so Pi's `ToolExecutionComponent` falls back to the built-in renderers by name. Previously the spread copied stale closures; now Pi resolves the renderer fresh from the built-in tool definition, showing the standard colored boxes with diff previews and syntax-highlighted content.
- pi: Migrate extension imports from deprecated `@mariozechner/pi-coding-agent` / `@mariozechner/pi-tui` to `@earendil-works/pi-coding-agent` / `@earendil-works/pi-tui`.

## [0.6.0] - 2026-05-09

### Added

- claude-code: Add Claude Code plugin
- diff: Add Terraform scope support. Edits inside a `block` (`resource "aws_s3_bucket" "logs"`, `variable "region"`, `module "vpc"`, …) anchor the hunk on the block and produce a breadcrumb naming the block by its type plus its string labels. Multi-line `tuple` (`[ … ]`) and `object` (`{ k = v … }`) literals act as data-tier scopes so the binding line and surrounding entries stay visible when an edit lands inside one of them — including the common case where the edit sits in a tuple/object inside a block too large to expand on its own.
- diff: A comment or attribute above a function/class now anchors the hunk on that function/class instead of its parent.

### Changed

- edit/write/traces: Rename the `summary` field to `reason` on edit/write requests, individual `edits[]`, and stored trace entries (CLI flag is now `--reason`). Old traces still load via a `summary` alias.
- traces: Trace ids may now be any safe string up to 128 chars (previously ULID-only), so external integrations can key traces on their own session ids (e.g. Claude Code's `session_id`).

## [0.5.0] - 2026-05-08

### Added

- Add `deltoids review` TUI for reviewing diffs.

### Changed

- Collapse the toolkit into a single `deltoids` binary with subcommands `pager`, `review`, `edit`, `write`, `traces`, shipped as one homebrew formula (`brew install juanibiapina/taps/deltoids`) and one shell installer. Plain `deltoids` (no subcommand) still runs the pager when stdin is piped, so existing `git config core.pager 'deltoids | less -R'` setups keep working.
- diff: Expand hunk context for multi-line literals so the binding line stays visible, matching the existing JSON/TS-config/YAML behaviour.
- diff: Anchor hunks on callbacks where the first argument is a string (`describe("…", () => {})`, `it "…" do … end`, `t.Run("…", func(…) {})`, `app.get("/…", () => {})`, `it("…", function() … end)`, …) and show the call in the breadcrumb so changes inside test cases, subtests, and route handlers locate themselves by suite/route even when the surrounding callback body is too large to fit in the hunk. Unlabeled callbacks (`xs.map(x => …)`, `Promise.then(…)`) still anchor on their enclosing named function as before.
- review: Remove the `Space` page-down shortcut (use `PgDn`).

## [0.4.0] - 2026-04-30

### Added

- Add `[theme] mode = "light" | "dark" | "auto"` to select the built-in light/dark palettes.

### Fixed

- Use the same light/dark mode for syntax-theme fallback and diff/UI colors.
- Fail with a clear `git fetch` hint when a diff references blobs missing from your local repo (e.g. `gh pr diff` for an unfetched PR). Previously rendered an empty or misleading diff.

## [0.3.0] - 2026-04-30

### Added

- Add public `Snapshot` and `DiffOp` types for line-level diffs without tree-sitter scope expansion. `Snapshot::compute(original, updated)` runs the line-level engine once and exposes `ops()`, `unified_text()`, and `align_old_to_new(line)`. `Diff::snapshot()` exposes the underlying snapshot.
- Detect languages from shebang lines, filenames, and modelines (e.g. a Bash file named `script`).

### Changed

- Hide `syntax::ParsedFile` internals behind a small interface. Public methods: `ParsedFile::parse(path, source)`, `enclosing_scopes(line)`, `is_structure(&scope)`, and `is_data(&scope)`.
- Extract the line-level diff engine from `scope.rs` into a private `engine` module. `Diff` now owns a `Snapshot` and delegates to it for raw ops and unified text. The scope planner and hunk builder consume `engine::DiffOp` instead of an inline copy.
- Swap the line-level diff backend from `similar` to `gix-imara-diff` with the Histogram algorithm and imara's line postprocessing. Hunks and unified text are produced by the same shared backend.

### Fixed

- Fix a bug where a `Delete` op spanning a partial scope plus one or more fully-deleted sibling scopes silently dropped every line beyond the first scope from the engine's hunks. The planner now walks the full delete range and emits one range per scope it intersects, so every removed line is accounted for in some hunk.
- Holding `j`/`k` in the trace browser TUI no longer falls behind the keyboard. The input loop now drains every buffered event into one batch and applies the whole batch before the next redraw, so a key-repeat burst collapses into a single redraw at the end of the burst instead of one redraw per repeat.

### Removed

- Remove the `syntax::parse_file` free function (replaced by `ParsedFile::parse`).
- Make `syntax::ParsedFile` internals private: `tree`, `structure_kinds`, `data_kinds`, `promoted_kinds`, and `function_body_kinds`.

## [0.2.0] - 2026-04-27

### Changed

- Rework the scope model into two tiers per language. Named code structures (functions, classes, tables, headings) anchor hunks with innermost strategy; data containers (JSON/TS `object`/`array`, YAML `block_mapping`/`block_sequence`) fall back with outermost-fit strategy. Small JSON/YAML diffs now render as a single hunk covering the whole root container. Breadcrumb boxes show only named structures, so JSON/YAML configs render with no box.
- Recognise JS/TS class fields and lexical declarations whose value is an arrow function or function expression as scopes. Changes inside `foo = async () => { ... }` (class field) or `const foo = () => { ... }` (top-level) anchor on `foo` for both the breadcrumb and the hunk range. Anonymous arrow callbacks and non-function fields are not promoted.

### Fixed

- Fix a bug where diff did not show when adding a line at end of file.
- Fix a bug where newly added files showed nested ancestor scope boxes.
- Fix a bug where a new sibling pair inserted next to a modified pair (JSON/YAML/JS/TS) was dropped from the rendered diff.
- Fix a bug where added lines could be lost when a Replace op spanned two top-level JSON pairs with different keys.
- Renames or arrow-to-method conversions after an earlier line-shifting edit no longer produce a duplicate hunk anchored on the new scope. Rename detection now uses diff-op alignment rather than absolute line position.
- Changes inside a method no longer climb to the enclosing class when the change spans more than one method or the inner method exceeds `MAX_SCOPE_LINES`. The hunk anchors on the inner method (or falls back to default 3-line context with the method as the breadcrumb) instead of leaking unrelated sibling methods into the hunk.
- Changes on method-level decorator lines (`@Cron(...)`, `@EventPattern(...)`, etc.) anchor on the method they decorate instead of the enclosing class. Class-level decorators (`@Injectable()`) anchor on the class.
- Functions declared inside another function body (e.g. `fn inner` inside `fn outer`, or `const inner = () => {}` inside a class method) are treated as local helpers and no longer steal the hunk anchor from the enclosing named container. Top-level functions, class methods, class arrow-fields, and top-level `const f = () => {}` continue to be anchors.

## [0.1.0] - 2025-04-23

Initial beta release.

### Added

- Add `deltoids`, a diff filter with tree-sitter scope context.
- Add `edit`, a file edit tool for coding agents.
- Add `write`, a file write tool for coding agents.
- Add `edit-tui`, a trace browser TUI.
- Add the pi plugin for agent integration.
