# Single line appended to plain text

## Why this case exists

The most basic non-trivial change. A two-line file gains a third line.
Plain text has no tree-sitter grammar registered, so the diff falls
back to unified context with no ancestor enrichment.

## Behaviours pinned

- One hunk, no ancestors.
- The unchanged first line shows up as a context line.
- The new line shows up as a `+` line.
