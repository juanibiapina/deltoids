# C `/* … */` comment-only edit above a fn anchors on the fn

## Why this case exists

C `/* … */` comments are parsed as `comment` siblings of
`function_definition`. A comment-only edit at the top level finds no
enclosing structure and today falls back to default 3-line context.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_definition bar]`.
- Hunk does not include sibling fns.
