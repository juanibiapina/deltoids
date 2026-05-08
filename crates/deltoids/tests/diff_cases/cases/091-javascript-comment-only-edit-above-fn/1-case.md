# JS line-comment-only edit above a top-level fn anchors on the fn

## Why this case exists

JavaScript and TypeScript share grammar but live as distinct languages
in deltoids' config. A doc-`//` comment above a top-level
`function_declaration` is a sibling, so a comment-only edit today
finds no enclosing structure and falls back to default 3-line context
with an empty breadcrumb.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_declaration bar]`.
- Hunk does not include sibling fns.
