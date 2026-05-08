# Multi-line Ruby hash literal expands hunk context

## Why this case exists

Top-level hash and array constants are everywhere in Ruby
(`config/routes.rb`, settings, lookup tables). A change inside one
should produce a hunk that includes the binding line, not just three
lines of unrelated entries. The engine treats `hash` and `array` as
data-tier scopes so the hunk grows to fit, mirroring the
JSON/TS-config/YAML pattern.

## Behaviours pinned

- A change inside a multi-line hash literal produces a hunk that
  spans the full literal, including the line that opens it.
- The hunk has no breadcrumb — `hash` is a data scope, not a
  structure.
