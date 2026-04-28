# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `deltoids`: `syntax::ParsedFile` now hides its tree-sitter `Tree`, source buffer, and language `*_kinds` tables behind a small interface. Public methods: `ParsedFile::parse(path, source)`, `enclosing_scopes(line)`, `is_structure(&scope)`, `is_data(&scope)`.

### Fixed

- `deltoids`: fix bug where a `Delete` op spanning a partial scope plus one or more fully-deleted sibling scopes silently dropped every line beyond the first scope from the engine's hunks. The planner now walks the full delete range and emits one range per scope it intersects, so every removed line is accounted for in some hunk.

### Removed

- `deltoids`: `syntax::parse_file` free function (replaced by `ParsedFile::parse`).
- `deltoids`: public fields on `syntax::ParsedFile` (`tree`, `structure_kinds`, `data_kinds`, `promoted_kinds`, `function_body_kinds`) — now private.

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
