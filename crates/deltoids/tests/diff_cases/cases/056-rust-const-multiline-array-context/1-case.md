# Multi-line Rust array literal expands hunk context

## Why this case exists

A change inside a multi-line top-level array literal (e.g.
`const KEYS: &[…] = &[ … ];`) should produce a hunk wide enough that
the binding line is visible. Without scope expansion the reader sees a
deleted tuple with three lines of unrelated entries above and below
and no clue what container it belongs to.

This mirrors the pattern already pinned for JSON, TS-config object
literals, and YAML mappings (cases 070, 075, 090): the engine treats
the literal as a data-tier scope and grows the hunk to fit the whole
literal.

## Behaviours pinned

- A change inside a multi-line `&[ … ]` produces a hunk that spans the
  full literal, including the line that opens the literal (here, the
  `const` declaration).
- The hunk has no breadcrumb — `array_expression` is a data scope,
  not a structure.
