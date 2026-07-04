# Nested outer/inner scope edits produce one hunk, not two

## Why this case exists

An edit on a direct child of an outer block (`desc`, a sibling of the
inner `task`) alongside edits inside a nested inner block (`task`) used
to emit **two overlapping hunks** that rendered the task body twice: an
outer hunk spanning the whole `namespace` and an inner hunk spanning the
whole `task`, with the inner nested inside the outer.

The outer edit anchors on `namespace` and expands to the whole block;
the inner edits anchor on `task` and expand to the whole task body.
Those two ranges are in an ancestor/descendant relationship, so they
overlap. `merge_ranges` must collapse overlapping (nested) ranges into a
single hunk regardless of scope identity, so no line is rendered twice.

## Behaviours pinned

- Nested outer/inner scope edits produce exactly **one** hunk.
- The hunk covers the whole outer block, and every changed line appears
  once (no duplicated task body).

## Note

The merged hunk's breadcrumb still names the inner scope
(`[call namespace] [call task]`) even though the `desc` change is not
inside `task`. That is a separate defect (breadcrumb should be the
lowest common ancestor of the changed lines) tracked as a follow-up and
intentionally left unchanged here.
