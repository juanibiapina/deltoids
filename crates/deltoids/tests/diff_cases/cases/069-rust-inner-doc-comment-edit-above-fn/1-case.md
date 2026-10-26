# Module docs edit directly above a function stays at the top level

## Why this case exists

Rust inner doc comments (`//!`, `/*!`) document the enclosing file or
module, never the item after them. tree-sitter-rust parses them as the
same `line_comment` / `block_comment` kinds as `///`, so a rule that
attaches comments above a definition by kind alone would hand module
docs to the first function when nothing (no `use`, no other code)
sits between them.

## Behaviours pinned

- One hunk covers the module-docs edit.
- The hunk has no `function_item` breadcrumb.
- The hunk does not expand to cover `parse`; it keeps default context.
