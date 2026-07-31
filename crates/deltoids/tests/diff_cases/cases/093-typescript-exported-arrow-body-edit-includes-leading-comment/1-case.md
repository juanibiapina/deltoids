# Body edit in an exported arrow function includes its leading comments

## Why this case exists

`export const f = (…) => { … }` is the dominant function shape in
TypeScript. A change inside the body must show the doc comment above
the declaration, exactly like the Rust `fn` case
(`065-rust-body-edit-includes-leading-doc-comment`).

The scope picked for this shape is the promoted `variable_declarator`,
which sits inside `lexical_declaration` inside `export_statement`. The
comment is a previous sibling of the `export_statement`, two levels up,
so a start extension that only looks at the scope node's own previous
siblings finds nothing and the comment is dropped. The extension has to
climb the wrappers that start on the same row as the scope first.

## Behaviours pinned

- One hunk covers the body edit.
- The hunk context starts at the first `//` comment line, not at
  `export const resolveContext = (`.
- The breadcrumb names the arrow-function scope.
