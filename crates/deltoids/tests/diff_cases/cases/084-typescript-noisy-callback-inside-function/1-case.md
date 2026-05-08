# Noisy inline callback inside a named function

## Why this case exists

Anonymous arrow functions are anchor-only kinds in JS/TS, so they
size hunks. That rule must NOT make every `xs.map(x => x + 1)` or
`Promise.then(v => …)` steal the anchor from the named function
that contains it. The expected behaviour: when an arrow callback
sits inside a named function body, the arrow is treated as a
local helper (demoted) and the hunk anchors on the function.

This case pins the negative side of the design: callbacks anchor
when they have no enclosing named structure (cases 082 / 083), but
they do not anchor when one already exists.

## Behaviours pinned

- A change inside an inline `xs.map(item => …)` callback inside a
  named function `compute` anchors on `compute`, not on the arrow.
- The hunk's breadcrumb is `[function_declaration compute]`.
- The hunk's context covers the body of `compute`.
