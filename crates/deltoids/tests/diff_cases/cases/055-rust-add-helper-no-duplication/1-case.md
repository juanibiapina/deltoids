# Adding a helper function next to a modified sibling does not duplicate the helper

## Why this case exists

A common refactor: a new helper (`fn visible_char`) is added and an
adjacent function (`fn truncate_ranges`) is updated to call it. A naive
implementation can include the new helper as *context* in the modified
sibling's hunk, making the helper appear twice in the rendered diff.

The engine must recognise that an inserted region contains a brand-new
named scope, give that scope its own hunk, and exclude it from the
sibling's context expansion.

## Behaviours pinned

- The new helper appears in exactly one hunk anchored on the new scope
  (`function_item visible_char` / `struct_item VisibleChar`).
- The hunk for the modified sibling (`fn truncate_ranges`) does **not**
  include `fn visible_char` as a context line.
- The new struct (`VisibleChar`) similarly appears in a single hunk.

## Notes

This is a regression test for a real bug. The `2-original.rs` and
`3-updated.rs` files are kept faithful to a real-world refactor (the
`src/highlight.rs` change that originally surfaced the bug) so the
behaviour is grounded in actual code, not a synthetic toy.
