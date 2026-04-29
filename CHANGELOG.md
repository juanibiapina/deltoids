# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `deltoids`: new public `Snapshot` and `DiffOp` types for line-level diffs without tree-sitter scope expansion. `Snapshot::compute(original, updated)` runs the line-level engine once and exposes `ops()`, `unified_text()`, and `align_old_to_new(line)`. `Diff::snapshot()` exposes the underlying snapshot.
- syntax highlighting now also detects languages from shebang lines, filenames, and modelines (e.g. a Bash file named `script`).

### Changed

- `deltoids`: `syntax::ParsedFile` now hides its tree-sitter `Tree`, source buffer, and language `*_kinds` tables behind a small interface. Public methods: `ParsedFile::parse(path, source)`, `enclosing_scopes(line)`, `is_structure(&scope)`, `is_data(&scope)`.
- `deltoids`: line-level diff engine extracted from `scope.rs` into a private `engine` module. `Diff` now owns a `Snapshot` and delegates to it for raw ops and unified text. The scope planner and hunk builder consume `engine::DiffOp` instead of an inline copy.
- `deltoids`: line-level diff backend swapped from `similar` to `gix-imara-diff` with the Histogram algorithm and imara's line postprocessing. Hunks and unified text are produced by the same shared backend.
- Marketing site at `deltoids.dev` rewritten in bare Astro under `site/` and moved back to GitHub Pages (was Next.js on Vercel under `web/`). Zero client JS, self-hosted IBM Plex Sans + JetBrains Mono, Lighthouse 100/100/100/100 mobile and desktop. The Next.js implementation under `web/` and its Vercel deploy workflow are removed.

### Fixed

- `deltoids`: fix bug where a `Delete` op spanning a partial scope plus one or more fully-deleted sibling scopes silently dropped every line beyond the first scope from the engine's hunks. The planner now walks the full delete range and emits one range per scope it intersects, so every removed line is accounted for in some hunk.
- `edit-tui`: holding `j`/`k` no longer falls behind the keyboard. The input loop now drains every buffered event into one batch and applies the whole batch before the next redraw, so a key-repeat burst collapses into a single redraw at the end of the burst instead of one redraw per repeat.
- `site`: the global stylesheet is now inlined into the HTML instead of loaded as an external file. The single ~10 KB CSS bundle was the only render-blocking request on the page; inlining it removes one round-trip and drops mobile LCP from ~2.6 s to ~1.7 s.

### Removed

- `deltoids`: `syntax::parse_file` free function (replaced by `ParsedFile::parse`).
- `deltoids`: public fields on `syntax::ParsedFile` (`tree`, `structure_kinds`, `data_kinds`, `promoted_kinds`, `function_body_kinds`) — now private.
- `website/`: Astro + Starlight landing site and its `.github/workflows/pages.yml` GitHub Pages deploy workflow. The Next.js site under `web/` (deployed to Vercel) is now the sole landing site for `deltoids.dev`.

## [0.2.0] - 2026-04-27

### Added

- Landing page at https://deltoids.dev

### Changed

- `deltoids`: rework scope model into two tiers per language. Named code structures (functions, classes, tables, headings) anchor hunks with innermost strategy; data containers (JSON/TS `object`/`array`, YAML `block_mapping`/`block_sequence`) fall back with outermost-fit strategy. Small JSON/YAML diffs now render as a single hunk covering the whole root container. Breadcrumb boxes show only named structures, so JSON/YAML configs render with no box.
- `deltoids`: JS/TS class fields and lexical declarations whose value is an arrow function or function expression are now recognised as scopes. Changes inside `foo = async () => { ... }` (class field) or `const foo = () => { ... }` (top-level) anchor on `foo` for both the breadcrumb and the hunk range. Anonymous arrow callbacks and non-function fields are not promoted.

### Fixed

- `deltoids`: fix bug where diff did not show when adding a line at end of file
- `deltoids`: fix bug where newly added files showed nested ancestor scope boxes
- `deltoids`: fix bug where a new sibling pair inserted next to a modified pair (JSON/YAML/JS/TS) was dropped from the rendered diff
- `deltoids`: fix bug where added lines could be lost when a Replace op spanned two top-level JSON pairs with different keys
- `deltoids`: a rename or arrow-to-method conversion that follows an earlier line-shifting edit no longer produces a duplicate hunk anchored on the new scope. Rename detection now uses diff-op alignment rather than absolute line position.
- `deltoids`: changes inside a method no longer climb to the enclosing class when the change spans more than one method or the inner method exceeds `MAX_SCOPE_LINES`. The hunk anchors on the inner method (or falls back to default 3-line context with the method as the breadcrumb) instead of leaking unrelated sibling methods into the hunk.
- `deltoids`: a change on a method-level decorator line (`@Cron(...)`, `@EventPattern(...)`, etc.) anchors on the method it decorates instead of the enclosing class. Class-level decorators (`@Injectable()`) anchor on the class.
- `deltoids`: a function declared inside another function body (e.g. `fn inner` inside `fn outer`, or `const inner = () => {}` inside a class method) is now treated as a local helper and no longer steals the hunk anchor from the enclosing named container. Top-level functions, class methods, class arrow-fields, and top-level `const f = () => {}` continue to be anchors.

## [0.1.0] - 2025-04-23

Initial beta release.

### Added

- `deltoids` - diff filter with tree-sitter scope context
- `edit` - file edit tool for coding agents
- `write` - file write tool for coding agents
- `edit-tui` - trace browser TUI
- pi plugin for agent integration
