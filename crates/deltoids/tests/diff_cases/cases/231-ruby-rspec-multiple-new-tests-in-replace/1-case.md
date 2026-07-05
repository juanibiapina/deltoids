# Ruby/RSpec: several new `it` blocks fused into one Replace

## Why this case exists

Extends case 230 to the multi-scope shape seen on the wild file that
first exposed the bug: one edited example plus **three** brand-new
examples, all fused by the line-level diff into a single `Replace` whose
new content carries every new test.

The builder renders the `Replace` faithfully — every new line as `+`,
every removed line as `-`, matched lines as context — so all three new
examples plus the edit render in one hunk whose `+/-` counts match git.
Nothing is dropped, nothing is duplicated, and the unchanged closing
`end` renders as context. The changed lines span several sibling `it`
blocks, so the breadcrumb is their shared parent `[call RSpec.describe]`.

## Behaviours pinned

- The edit and all three new examples (`raises when recovery returns
  nothing`, `retries on transient errors`, `logs the recovery attempt`)
  render together in one hunk whose `+/-` counts match `git diff`.
- Every added line appears exactly once; none is dropped or duplicated.
- The unchanged `it "exposes the total count"` example stays out of the
  hunk (it renders only the closing `end` as context).
- The breadcrumb names the shared parent `[call RSpec.describe]`.
