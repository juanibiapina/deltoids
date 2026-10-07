# Expanding a new method's hunk shows its enclosing impl

## Why this case exists

A whole new method is shown as its own hunk with no context. Expanding it
shows where it landed: the hunk grows to the enclosing `impl`.

## Behaviours pinned

- The new method starts as an isolated, context-free hunk.
- Expanding it grows the context to the whole `impl` around the
  insertion point.
