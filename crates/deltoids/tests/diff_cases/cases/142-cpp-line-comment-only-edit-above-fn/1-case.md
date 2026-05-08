# C++ `//` comment-only edit above a fn anchors on the fn

## Why this case exists

C++ `//` line comments are parsed as `comment` siblings of
`function_definition`. The fix must cover the C++ comment kind.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_definition bar]`.
- Hunk does not include sibling fns.
