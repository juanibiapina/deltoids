# Ruby/RSpec: a new `it` block fused into a Replace op is not dropped

## Why this case exists

Reproduces a data-loss bug. When an existing RSpec `it "…" do … end`
example is edited (renamed + expectation changed) **and** a brand-new
`it "…" do … end` example is added right after it, the line-level diff
engine fuses the edit and the insertion into `Replace` ops whose new
side carries the whole new test. The engine used to render **only** the
edited example and silently drop every line of the new one.

Concretely, `Snapshot::compute` produces:

```
Replace { old_index: 1, old_len: 1, new_index: 1, new_len: 1 }   # rename `it` line
Replace { old_index: 3, old_len: 1, new_index: 3, new_len: 6 }   # changed expect + entire new test
```

The second `Replace` replaces one old line with six new lines: the
rewritten `expect(...)` line plus the four lines of the brand-new
`it "raises when recovery returns nothing"` test (and a blank).

## Root cause (fixed)

Two collaborating pieces of the scope engine conspired to erase the
addition:

1. `hunk_builder::collect_replace_added_lines` truncates the added side
   of a `Replace` at the first *new scope* it detects. Correct on its
   own — the new test should live in its own hunk.

2. The Replace new-side planner bailed whenever the innermost structure
   it found on the new side was *anchor-only*. For RSpec, `it "…" do …
   end` is a call-promoted `call` wrapping an anonymous `do_block`; the
   innermost structure is that anchor-only `do_block`, so the guard
   fired and **no range was planned** for the new test. The added lines
   were truncated in step 1 and never re-collected in step 2 → dropped.

The fix routes new-scope detection through
`ParsedFile::named_scope_at`, which climbs from an anchor-only block to
its enclosing **labeled** call-promoted structure (the `it("…")` call).
The planner emits one add-only hunk per brand-new named scope in the
Replace's new content, and the aligned-edit hunk's cutoff uses the same
primitive so the two sides never overlap.

Originally observed on
`spec/lib/github/http_issues_client_spec.rb` (juanibiapina/ruby_test_task-master),
where one edited example plus four new examples collapsed into a single
hunk that showed only removals — the entire added region vanished.

## Behaviours pinned

- The renamed/edited example renders as one hunk (removals + adds).
- The brand-new `it "raises when recovery returns nothing"` example
  appears as its own add-only hunk — it is not dropped.

The trailing `[call expect]` breadcrumb in the second hunk is the
pre-existing unlabeled-call-promoted breadcrumb noise (see the
out-of-scope note in the fix plan); the load-bearing assertion is that
the new test's lines are all present.
