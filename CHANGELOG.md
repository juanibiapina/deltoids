# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `deltoids`: fix bug where diff did not show when adding a line at end of file
- `deltoids`: fix bug where newly added files showed nested ancestor scope boxes
- `deltoids`: fix bug where a new sibling pair inserted next to a modified pair (JSON/YAML/JS/TS) was dropped from the rendered diff

## [0.1.0] - 2025-04-23

Initial beta release.

### Added

- `deltoids` - diff filter with tree-sitter scope context
- `edit` - file edit tool for coding agents
- `write` - file write tool for coding agents
- `edit-tui` - trace browser TUI
- pi plugin for agent integration
