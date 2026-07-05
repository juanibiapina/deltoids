# Ruby/RSpec: identical stub bodies do not duplicate a line

## Why this case exists

Guards against a double-render bug. An existing RSpec `it "…" do … end`
example is renamed and its expectation rewritten, **and** a brand-new
`it "…" do … end` example is added whose stub body is byte-identical to
the edited example's stub body. Because the bodies match, the
line-level diff cross-matches the old body against the *new* example's
body, so the new example is split across diff ops.

Concretely, `Snapshot::compute` produces:

```
Equal   { old_index: 0,  new_index: 0,  len: 1 }
Replace { old_index: 1,  old_len: 1, new_index: 1,  new_len: 10 }
Equal   { old_index: 2,  new_index: 11, len: 5 }   # identical stub lines, cross-matched
Replace { old_index: 7,  old_len: 1, new_index: 16, new_len: 1 }
Equal   { old_index: 8,  new_index: 17, len: 2 }
```

The builder renders each op exactly once by its kind, so the shared stub
body renders as context (matched, exactly like git) and the differing
`expect { collect }.to raise_error(RecoveryError)` line renders as a
single `+`. No line is emitted twice, and the `+/-` counts match git.
The changed lines span two sibling `it` blocks, so the breadcrumb is
their shared parent `[call RSpec.describe]`.

Originally observed on
`spec/lib/github/http_issues_client_spec.rb` (juanibiapina/ruby_test_task-master,
commit `19bcf03`), where several new examples shared byte-identical stub
bodies and a two-line tail once rendered twice.

## Behaviours pinned

- The whole edit renders in one hunk whose `+/-` counts match `git
  diff`.
- The byte-identical stub body renders as context (matched), not as an
  addition.
- No line appears more than once; the cross-matched tail is rendered
  exactly once.
- The breadcrumb names the shared parent `[call RSpec.describe]`.
