# Merged hunk breadcrumb names the lowest common ancestor

## Why this case exists

When one hunk merges changes at different scope depths, the breadcrumb
must name the **lowest common ancestor** of the changed lines, not the
deepest single chain.

Here a `class Outer` has a `limit` field (a direct child of the class,
sibling of the `run` method) and a `run` method body. Editing both the
field and a line inside `run` merges the two ranges into one hunk
spanning the whole class. The field change is **not** inside `run`, so
the breadcrumb must name only `[class_declaration Outer]`.

## Behaviours pinned

- The two edits merge into a single hunk covering the whole class.
- The breadcrumb is the LCA of the changed lines
  (`[class_declaration Outer]`), not the inner
  `[method_definition run]`.
