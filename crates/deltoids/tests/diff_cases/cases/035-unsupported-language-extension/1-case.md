# Unsupported language extension falls back to plain unified diff

## Why this case exists

When the file extension does not map to a registered tree-sitter
grammar, `Diff::compute` cannot enrich hunks with ancestor scopes. It
must still produce a working unified diff with empty ancestor chains.

## Behaviours pinned

- Unknown extensions (e.g. `.xyz`) do not error.
- Hunks have no ancestor breadcrumb.
- The change is reported with standard 3-line context, identical to the
  plain-text path.
