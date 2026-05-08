# Change inside a Jest `it(...)` callback

## Why this case exists

Jest/Vitest tests express the relevant unit of behaviour as
`it("name", () => { ... })`. The `it` call's arrow-function body is
where the change lives, but the arrow has no syntactic name; the
identity of the test sits on the surrounding `call_expression`
(the callee `it` plus the string-literal label).

Today, when a change sits inside an object literal inside an `it`
callback, the engine anchors only on the inner `object` data scope
and the breadcrumb is empty — the reader cannot tell which test is
being changed.

We want:

1. The hunk to anchor on the `it` callback's arrow-function body so
   the call signature line is visible as the first context line.
2. The breadcrumb to identify the surrounding `describe(...)` and
   `it(...)` calls by name, so the change locates itself in the
   suite even when the hunk is small.

## Behaviours pinned

- The hunk anchors on the inner `it` callback's body, not on the
  inner `const scope = { ... }` literal.
- The hunk's breadcrumb is
  `[call_expression describe("UserService")]`
  `[call_expression it("creates a user")]`.
- Anonymous arrow functions are anchor-only and do not appear in
  the breadcrumb; only the labeled `call_expression` does.
