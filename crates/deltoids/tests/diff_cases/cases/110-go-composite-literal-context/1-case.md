# Multi-line Go composite literal expands hunk context

## Why this case exists

Package-level slice, map, and struct literals (`var X = []T{ … }`,
`var X = map[K]V{ … }`, `var X = T{ … }`) are common in Go and are
typically multi-line. A change inside one should produce a hunk that
includes the binding line, not just three lines of unrelated entries.

The engine treats `composite_literal` as a data-tier scope so the hunk
grows to fit, mirroring the JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line composite literal produces a hunk that
  spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `composite_literal` is a data scope,
  not a structure.
