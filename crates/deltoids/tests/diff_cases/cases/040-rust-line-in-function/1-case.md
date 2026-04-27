# Modifying a line inside a Rust function

## Why this case exists

The simplest "scope context" demonstration. A line inside a top-level
`fn` is changed; the engine must anchor the hunk on the enclosing
function so the breadcrumb chain reads `[function_item compute]`.

## Behaviours pinned

- Hunks anchored on a single Rust function carry exactly one ancestor.
- The ancestor's `kind` is `function_item` and its `name` matches the
  function name.
- The hunk expands its context to the whole function body (it is well
  under `MAX_SCOPE_LINES`), not the default 3-line radius.
