# Go doc-comment-only edit above a fn anchors on the fn

## Why this case exists

Go's idiomatic `// Foo …` doc comment above a top-level fn is a
`comment` sibling of `function_declaration`. A comment-only edit today
finds no enclosing structure and falls back to default 3-line context
with no breadcrumb.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_declaration bar]`.
- Hunk does not include sibling fns.
