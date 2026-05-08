# Multi-line C initializer list expands hunk context

## Why this case exists

C codebases use top-level array initializers as lookup tables
(keymaps, command tables, device descriptors, …). A change inside one
should produce a hunk that includes the binding line, not just three
lines of unrelated entries. The engine treats `initializer_list` as a
data-tier scope so the hunk grows to fit, mirroring the
JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line `{ … }` initializer produces a hunk
  that spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `initializer_list` is a data scope,
  not a structure.
