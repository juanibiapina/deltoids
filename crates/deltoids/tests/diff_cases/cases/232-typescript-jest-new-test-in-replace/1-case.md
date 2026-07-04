# TypeScript/Jest: a new `it(...)` test fused into a Replace op

## Why this case exists

Cross-language proof for the same bug as case 230. A Jest/Mocha
`it("…", () => {})` is a call-promoted `call_expression` wrapping an
anonymous `arrow_function`. When an existing test is edited and a new
`it(...)` is added right after, the line-level diff fuses them into a
`Replace`; new-scope detection must climb from the anonymous arrow to
the labeled `it("…")` call and give the new test its own hunk instead of
dropping it.

## Behaviours pinned

- The edited `it("recovers via the slow path", …)` test renders as one
  hunk (removals + adds).
- The brand-new `it("raises when recovery returns nothing", …)` test
  renders as its own complete add-only hunk.
