# Nearby changes in large function merge across data containers

## Why this case exists

When a function exceeds MAX_SCOPE_LINES (200), multiple changes within
a ~20-line stretch should merge into one hunk, just like they do in a
small function. Currently, each change near a different data container
(object literal) gets a distinct `scope_id` equal to that container's
bounds. The merge logic refuses to combine ranges with different
`scope_id`s, so changes that are only a few lines apart produce
separate hunks with redundant scope headers and overlapping context.

This reproduces the "same scope header repeated" bug when reviewing
diffs of large functions with scattered changes.

## Behaviours pinned

- Three replaces in adjacent object literals, all within ~15 lines
  inside a 200+ line function, produce **one** merged hunk.
- The single hunk has the function as its breadcrumb ancestor.
- No line appears in more than one hunk.
