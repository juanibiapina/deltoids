# Lua `--` comment-only edit above a function anchors on the function

## Why this case exists

Lua `--` comments are parsed as `comment` siblings of
`function_declaration`. A comment-only edit at the chunk level today
finds no enclosing structure and falls back to default 3-line context.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_declaration bar]`.
- Hunk does not include sibling fns.
