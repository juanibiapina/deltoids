# Change inside an RSpec `it ... do ... end` block

## Why this case exists

RSpec is Ruby's iconic example of the labeled-callback pattern:
`describe "Subject" do ... it "name" do ... end ... end`. The `it`
call's `do_block` is where the change lives, but the block has no
syntactic name; the identity of the example sits on the
surrounding `call` (the method `it` plus its string-literal first
argument).

Today, when a change sits inside a hash literal inside an `it` block,
the engine anchors only on the inner `hash` data scope and the
breadcrumb is empty — the reader cannot tell which example is being
changed.

This is the Ruby equivalent of case 082 (TypeScript Jest).

## Behaviours pinned

- The hunk anchors on the inner `it` block, not on the inner
  `scope = { ... }` hash literal.
- The hunk's breadcrumb is
  `[call RSpec.describe("UserService")]`
  `[call it("creates a user")]`.
- Anonymous Ruby blocks (`do_block`, `block`) are anchor-only and do
  not appear in the breadcrumb; only the labeled `call` does.
