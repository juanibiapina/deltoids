# Top-level Rust statement has no breadcrumb ancestor

## Why this case exists

Not every change sits inside a named structure. Top-level `const`,
`static`, or `let` bindings live directly in the source file with no
enclosing `fn`/`impl`/`struct`. The engine must produce hunks with an
empty ancestor chain in that case, not invent a synthetic root.

## Behaviours pinned

- A change at the top level produces a hunk with no breadcrumb.
- The hunk uses default 3-line context (no scope to expand to).
