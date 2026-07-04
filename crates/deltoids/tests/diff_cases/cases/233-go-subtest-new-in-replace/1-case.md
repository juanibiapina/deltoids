# Go: a new `t.Run(...)` subtest fused into a Replace op

## Why this case exists

Second cross-language proof for the case 230 bug. A Go subtest
`t.Run("…", func(t *testing.T) {})` is a call-promoted `call_expression`
wrapping an anonymous `func_literal`. When an existing subtest is edited
and a new `t.Run(...)` is added right after, the line-level diff fuses
them into a `Replace`; new-scope detection must climb from the anonymous
`func_literal` to the labeled `t.Run("…")` call and give the new subtest
its own hunk instead of dropping it.

## Behaviours pinned

- The edited `t.Run("recovers via the slow path", …)` subtest renders as
  one hunk (removals + adds).
- The brand-new `t.Run("raises when recovery returns nothing", …)`
  subtest renders as its own complete add-only hunk.
