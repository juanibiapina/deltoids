# An insert between two adjacent functions appears in one hunk

## Why this case exists

`fn a` is edited and a `const` is inserted between `a`'s closing brace and
`fn b`, with no blank line between the functions. The insert sat on the
edge of both functions' hunks, so the engine used to show the added line
in both.

## Behaviours pinned

- Two hunks: the edit inside `a`, and the insert anchored on `b`.
- The added `const` appears exactly once, and the hunks do not overlap.
