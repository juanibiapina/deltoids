# Comment-only edit above a method should anchor on the method

## Why this case exists

A `///` doc comment that sits above a method is, in tree-sitter, a
sibling of the `function_item` and a child of the surrounding
`declaration_list` / `impl_item`. A line query at the comment line
returns the impl block as the only enclosing structure, never the
method below.

Case `060-rust-comment-anchor-inside-fn` already pins the related
scenario where the same hunk *also* contains a body edit: the body
edit's anchor candidate provides the function ancestor and the hunk
breadcrumb correctly reads `[function_item compute]`.

This case pins the **comment-only** scenario, where the only changed
line is the doc comment itself. Today the engine has no body-edit
candidate to fall back on, so it anchors on the impl block, expands the
hunk to cover the impl, and the breadcrumb reads `[impl_item Foo]`
instead of `[impl_item Foo] [function_item bar]`.

The expected behaviour: a hunk anchored on a doc comment should
promote to the immediately following named structure, the same way
case `060` does, regardless of whether the hunk also contains a body
edit.

## Behaviours pinned

- One hunk covers the doc-comment edit only.
- The hunk's ancestor chain is `[impl_item Foo] [function_item bar]`.
- The hunk does **not** expand to cover unrelated siblings of `bar`
  inside the same `impl`.

## Notes

When this case is fixed, the recorded `4-expected.diff` will show a
small hunk with the two-level breadcrumb. Until then, refreshing the
case via `DELTOIDS_UPDATE_CASES=1` records today's broken output —
which is useful as a starting baseline but should be reviewed by hand
before committing.
