# Block-comment-only edit above a Rust fn anchors on the fn

## Why this case exists

`/* … */` is parsed as `block_comment` (distinct kind from
`line_comment`). The fix for leading comments must list both kinds.

## Behaviours pinned

- Editing only the block comment above `fn bar` produces one hunk
  whose ancestor chain is `[function_item bar]`.
- The hunk does not include sibling fns.
