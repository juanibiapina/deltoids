# Ruby/RSpec: a new `it` block fused into a Replace op is rendered faithfully

## Why this case exists

Guards against a data-loss bug. When an existing RSpec `it "…" do … end`
example is edited (renamed + expectation changed) **and** a brand-new
`it "…" do … end` example is added right after it, the line-level diff
fuses the edit and the insertion into a `Replace` op whose new side
carries the whole new test.

The hunk builder renders every line of a `Replace` exactly by its op
kind — removed lines as `-`, new lines as `+`, matched lines as
context — so the displayed line counts equal git's. The whole edit
renders as a **single** hunk closing on the unchanged `end` as context.
Because the hunk's changed lines span two sibling `it` blocks, its
breadcrumb is their shared parent `[call RSpec.describe]`, not either
individual example.

Originally observed on
`spec/lib/github/http_issues_client_spec.rb` (juanibiapina/ruby_test_task-master),
where one edited example plus new examples once collapsed into a hunk
that showed only removals — the entire added region vanished.

## Behaviours pinned

- The edit and the new example render together in one hunk whose `+/-`
  counts match `git diff`.
- Every added line of the new `it "raises when recovery returns
  nothing"` example appears exactly once; no line is dropped.
- The unchanged closing `end` renders as context, not as an addition.
- The breadcrumb names the shared parent `[call RSpec.describe]`.
