# Modifying a line inside a method on an `impl` block

## Why this case exists

When a change sits inside a method on an `impl` block, the breadcrumb
should describe both layers of nesting: the `impl Foo` outer block and
the `compute` method inside it. This case proves the ancestor chain is
populated outermost-first.

## Behaviours pinned

- Two ancestors appear: `[impl_item Foo]` then `[function_item compute]`.
- The hunk's context expansion stays within the method, anchored on
  the innermost named structure.
