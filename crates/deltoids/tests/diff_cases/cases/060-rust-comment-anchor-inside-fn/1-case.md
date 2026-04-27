# Doc comment change above a function keeps the function as ancestor

## Why this case exists

A doc comment that sits *above* a function (`/// docs\nfn name() {...}`)
is, in the tree-sitter grammar, a sibling of the function — not a
child. A naive lookup for the ancestor scope at the comment line would
walk straight to the file root and report no ancestor.

The engine fixes this by promoting comment-anchored hunks to their
following named structure when a function-affecting change is in the
same hunk, so the breadcrumb correctly reads `[function_item compute]`.

## Behaviours pinned

- One hunk covers both the doc-comment edit and the body edit.
- The hunk's ancestor chain contains a single entry,
  `[function_item compute]`.
- The doc comment line appears as `-`/`+` inside that hunk.

## Notes

This is the small reproducer for the historical
`scope_comment_anchor` regression test, which used a much larger
fixture taken from `scope.rs` itself. Both pin the same behaviour; the
smaller example here is easier to review.
