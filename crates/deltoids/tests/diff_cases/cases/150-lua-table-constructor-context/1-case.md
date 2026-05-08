# Multi-line Lua table constructor expands hunk context

## Why this case exists

Lua codebases (Neovim configs, LÖVE2D games, OpenResty modules) lean
heavily on top-level multi-line table constructors for settings and
keymaps. A change inside one should produce a hunk that includes the
binding line, not just three lines of unrelated entries. The engine
treats `table_constructor` as a data-tier scope so the hunk grows to
fit, mirroring the JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line `{ … }` table produces a hunk that
  spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `table_constructor` is a data scope,
  not a structure.
