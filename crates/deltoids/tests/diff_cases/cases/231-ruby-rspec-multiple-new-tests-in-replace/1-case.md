# Ruby/RSpec: several new `it` blocks fused into one Replace

## Why this case exists

Extends case 230 to the multi-scope shape seen on the wild file that
first exposed the bug: one edited example plus **three** brand-new
examples, all fused by the line-level diff into a single `Replace` whose
new content carries every new test.

A single-range Replace planner could only ever recover the first new
scope, dropping the rest. This case pins the per-scope cursor loop: the
planner must emit one add-only hunk **per** brand-new labeled callback,
and each hunk must contain the whole test (no header-only fragments, no
duplicated lines, no stray leading blanks).

## Behaviours pinned

- The edited `it "recovers via the slow path"` example renders as one
  hunk (removals + adds).
- Each of the three new examples (`raises when recovery returns
  nothing`, `retries on transient errors`, `logs the recovery attempt`)
  renders as its own complete add-only hunk.
- The unchanged `it "exposes the total count"` example does not appear.
