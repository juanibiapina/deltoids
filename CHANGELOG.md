# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
