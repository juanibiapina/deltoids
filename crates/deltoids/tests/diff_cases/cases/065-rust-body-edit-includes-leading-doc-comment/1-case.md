# Body edit in a function with leading doc comments and attributes includes them

## Why this case exists

When a function has `///` doc comments and an `#[inline]` attribute
above it and only the function body is changed, the scope expansion
should include both the leading doc comments and the attribute as part
of the context. Doc comments and attributes are integral to a
function's definition and omitting them from the hunk context loses
important information for the reader.

In tree-sitter, doc comments are siblings of the `function_item`, not
children, and `#[…]` attributes are `attribute_item` siblings. The
`function_item` node's start line is the `fn` keyword. The scope
expansion must look back past the `fn` line to include contiguous
leading comments and attributes.

## Behaviours pinned

- One hunk covers the body edit.
- The hunk's ancestor chain is `[function_item compute]`.
- The hunk context starts at the `///` doc comment, not at `fn compute`,
  and includes the `#[inline]` attribute.
