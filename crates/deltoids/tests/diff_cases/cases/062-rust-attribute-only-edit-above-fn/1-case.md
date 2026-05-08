# Attribute-only edit above a Rust fn anchors on the fn

## Why this case exists

A Rust outer attribute (`#[inline]`, `#[derive(...)]`) above a function
is parsed as an `attribute_item` sibling of the `function_item`, not a
child. A query at the attribute line returns no enclosing structure
(top-level fn) or only the parent `impl`/`mod` (nested fn).

Sister case to `061-rust-comment-only-edit-above-method` for doc
comments. Both must promote a leading-comment / attribute anchor to
the next sibling structure.

## Behaviours pinned

- Editing only the attribute line above `fn bar` produces one hunk
  whose ancestor chain is `[function_item bar]`.
- The hunk covers the attribute line and the body of `bar` only — not
  the sibling fns `other` and `another`.
