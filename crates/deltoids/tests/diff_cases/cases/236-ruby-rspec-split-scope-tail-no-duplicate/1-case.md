# Ruby/RSpec: a new scope split across ops does not duplicate its tail

## Why this case exists

Reproduces a double-render bug. An existing RSpec `it "…" do … end`
example is renamed and its expectation rewritten, **and** a brand-new
`it "…" do … end` example is added whose stub body is byte-identical to
the edited example's stub body. Because the bodies match, the
line-level diff cross-matches the old body against the *new* example's
body and splits that new example across diff ops: the new example's
head lands in one `Replace`, but its tail (the differing
`expect { … }` line) lands in a **later** `Replace` op that also
belongs to the edited example's old range.

Concretely, `Snapshot::compute` produces:

```
Equal   { old_index: 0,  new_index: 0,  len: 1 }
Replace { old_index: 1,  old_len: 1, new_index: 1,  new_len: 10 }  # rename head + first new test
Equal   { old_index: 2,  new_index: 11, len: 5 }                   # identical stub lines, cross-matched
Replace { old_index: 7,  old_len: 1, new_index: 16, new_len: 1 }   # TAIL of the second new test
Equal   { old_index: 8,  new_index: 17, len: 2 }
```

The `it "raises on a userless success"` scope spans new lines 10..17.
Its `expect { collect }.to raise_error(RecoveryError)` line (new 16) is
carried by the last `Replace` op, which falls inside the old edited
example's range.

## Root cause (fixed)

Two places decided "is this NEW line owned by a fresh new scope" and
disagreed. The planner (`new_replace_scope_ranges`) records the **full
span** of each fresh scope, independent of which op carries which part.
The builder (`first_different_new_scope_start`) re-derived ownership per
op and only recognised a scope whose **start** landed inside the current
op's new range. When a later op carried only a scope's tail, the builder
missed it and the aligned-edit hunk re-rendered the tail, so
`expect { collect }.to raise_error(RecoveryError)` appeared in **two**
hunks.

The fix makes the builder consult the planner's already-computed
new-scope spans: it skips any NEW line that falls inside any claimed
span, so a scope's tail is owned by exactly one hunk (its own add-only
new-scope-span hunk).

Originally observed on
`spec/lib/github/http_issues_client_spec.rb` (juanibiapina/ruby_test_task-master,
commit `19bcf03`), where several new examples shared byte-identical stub
bodies and a two-line tail rendered twice.

## Behaviours pinned

- The renamed/edited example renders as one hunk: its removals plus its
  edited `expect(...)` context, with **no** added `expect { … }` tail
  leaking in from the new example.
- Each brand-new `it` example renders as its own complete add-only hunk.
- No line appears in more than one hunk.
