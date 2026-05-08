# Multi-line C++ initializer list expands hunk context

## Why this case exists

Same idiom as C (case 130): top-level array initializers used as
lookup tables. The engine treats `initializer_list` as a data-tier
scope in C++ so a change inside a multi-line `{ … }` initializer
expands the hunk to cover the full literal, mirroring the
JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line `{ … }` initializer produces a hunk
  that spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `initializer_list` is a data scope,
  not a structure.
