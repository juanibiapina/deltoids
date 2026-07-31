# Body edit in an exported function declaration includes its leading comments

## Why this case exists

`export function f() { … }` wraps the `function_declaration` in an
`export_statement`, so the doc comment above it is a sibling of the
wrapper rather than of the function. Without climbing to the wrapper
the comment is dropped from the hunk, even though the same file without
`export` keeps it (case `091`).

Sibling of `093`, which pins the exported-arrow shape.

## Behaviours pinned

- One hunk covers the body edit.
- The hunk context starts at the first `//` comment line, not at
  `export function resolveContext(`.
- The breadcrumb names the function.
