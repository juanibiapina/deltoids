# Deleting a struct's closing brace and the next item shows every removed line

## Why this case exists

One delete removes a struct's closing `}`, the blank line after it, and
the top-level `const` that follows. The engine used to anchor the hunk on
the struct and show only `-}`; the blank line and the `const` were
silently dropped because no scope covered them.

## Behaviours pinned

- Every deleted line appears as `-`, exactly once.
- The hunk keeps the struct as context and as its breadcrumb.
