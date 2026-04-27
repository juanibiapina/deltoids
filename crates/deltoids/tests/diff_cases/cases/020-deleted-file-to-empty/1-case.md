# Deleted file: non-empty original, empty after

## Why this case exists

The companion to case 015. When the "after" content is empty (a deleted
file), the engine must still produce a single hunk that lists every
removed line. Scope detection on the empty side has nothing to anchor
against, so the hunk should have no ancestors.

## Behaviours pinned

- A non-empty `before` against an empty `after` produces one hunk.
- Every line of the deleted file appears as `-`.
- The hunk has no ancestor breadcrumb.

## Notes

`Diff::compute` parses both sides with tree-sitter when both are
non-empty. Because the new side is empty here, it falls back to plain
unified diff (no scope expansion), which is the behaviour we want.
