# Multi-line Rust struct literal expands hunk context

## Why this case exists

Top-level configuration bindings often use a multi-line struct
literal: `const CONFIG: Settings = Settings { … };`. A change to one
field should produce a hunk wide enough to show the binding line and
the surrounding fields, not just three lines of unrelated fields.

The engine treats `struct_expression` as a data-tier scope, so the
hunk grows to fit the whole literal. Same pattern as JSON, TS-config
object literals, and YAML mappings (cases 070, 075, 090).

## Behaviours pinned

- A change inside a multi-line `Foo { … }` literal produces a hunk
  that spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `struct_expression` is a data scope,
  not a structure.
