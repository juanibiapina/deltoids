# Ruby `#` comment-only edit above a method anchors on the method

## Why this case exists

A `#` comment above a Ruby method is a `comment` sibling of `method`,
child of the body list inside `class`. Today the comment-only edit
anchors on `class` and the hunk covers the whole class.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[class Foo] [method bar]`.
- Hunk does not include sibling methods.
