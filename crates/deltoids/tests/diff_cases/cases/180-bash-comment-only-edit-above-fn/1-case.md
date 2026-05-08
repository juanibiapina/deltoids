# Bash `#` comment-only edit above a function anchors on the function

## Why this case exists

A Bash `#` comment above a `function_definition` is a sibling of the
function in tree-sitter. A comment-only edit at the script level today
finds no enclosing structure and falls back to default 3-line context.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_definition bar]`.
- Hunk does not include sibling fns.
