# Python `#` comment-only edit above a method anchors on the method

## Why this case exists

A `#` comment above a Python method is a `comment` sibling of
`function_definition`, child of `block` inside `class_definition`.
Today the comment-only edit anchors on `class_definition` and the hunk
covers all sibling methods.

(Python decorators `@…` already get the right anchor through
`skip_decorators`. This case pins the comment shape, not decorators.)

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[class_definition Foo] [function_definition bar]`.
- Hunk does not include sibling methods.
