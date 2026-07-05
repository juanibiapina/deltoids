# TypeScript/Jest: a new `it(...)` test fused into a Replace op

## Why this case exists

Cross-language proof for the same shape as case 230. A Jest/Mocha
`it("…", () => {})` is a call-promoted `call_expression` wrapping an
anonymous `arrow_function`. When an existing test is edited and a new
`it(...)` is added right after, the line-level diff fuses them into a
`Replace`.

The builder renders the `Replace` faithfully: every new line is `+`,
every removed line is `-`, matched lines are context, so the `+/-`
counts match git. The edit and the new test render as one hunk closing
on the unchanged `});` as context. Its changed lines span two sibling
`it(...)` calls, so the breadcrumb is their shared parent
`[call_expression describe("client")]`.

## Behaviours pinned

- The edit and the brand-new `it("raises when recovery returns
  nothing", …)` test render in one hunk whose `+/-` counts match
  `git diff`.
- Every added line of the new test appears exactly once; none is
  dropped or duplicated.
- The breadcrumb names the shared parent
  `[call_expression describe("client")]`.
