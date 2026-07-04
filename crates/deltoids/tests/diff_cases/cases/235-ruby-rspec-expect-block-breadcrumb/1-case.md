# Ruby/RSpec: a hunk inside an `expect { … }` block does not name `[call expect]`

## Why this case exists

`expect { … }.to raise_error(...)` is a Ruby `call` with a block
argument, so the scope engine call-promotes it. But it carries no
identifying argument (the block is its only argument), so it is a
restructured expression, not a named unit of behaviour. It must not
appear as a breadcrumb boundary.

When **every** changed line of a hunk sits inside the same
`expect { … }` block, the lowest-common-ancestor breadcrumb reduction
(Bug 1b) cannot drop `expect`: it is present in every changed line's
ancestor chain, so the LCA keeps it. The breadcrumb filter must exclude
block-only call-promoted wrappers structurally.

Here both edited lines (`collect_pages(max)` -> `collect_pages(limit)`
and `finalize` -> `finalize!`) live inside the multi-line `expect { … }`
block, so the LCA alone would render `[call expect]`.

## Behaviours pinned

- The hunk's breadcrumb reads
  `[call RSpec.describe] [call it("raises on repeated failure")]` with
  **no** trailing `[call expect]`.
- `RSpec.describe` (constant argument) and `it("…")` (string argument)
  are kept: they carry an identifying argument, unlike bare `expect`.
