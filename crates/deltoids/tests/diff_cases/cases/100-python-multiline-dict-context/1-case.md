# Multi-line Python dict literal expands hunk context

## Why this case exists

Module-level config dictionaries (URL routes, shortcut tables, settings
maps) are common in Python. A change inside one should produce a hunk
that includes the binding line, not just three lines of unrelated
entries. The engine treats `dictionary` and `list` as data-tier scopes
so the hunk grows to fit, mirroring the JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line dict literal produces a hunk that spans
  the full literal, including the line that opens it.
- The hunk has no breadcrumb — `dictionary` is a data scope, not a
  structure.
