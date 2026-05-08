# Change inside a Go `t.Run(...)` subtest callback

## Why this case exists

Table-driven Go tests use `t.Run("subtest name", func(t *testing.T)
{ ... })` to declare named subtests. The subtest's `func_literal` is
the unit of behaviour, but the literal has no syntactic name; the
identity of the subtest sits on the surrounding `call_expression`
(the callee `t.Run` plus its string-literal label).

Today, when a change sits inside a composite literal inside a
subtest, the engine anchors on the outer test function and the
breadcrumb omits the subtest name — the reader cannot tell which
subtest is being changed.

This is the Go equivalent of case 082 (TypeScript Jest).

## Behaviours pinned

- The hunk anchors on the inner subtest's `func_literal` body, not
  on the inner composite literal or on the outer test function.
- The hunk's breadcrumb is
  `[function_declaration TestUserService]`
  `[call_expression t.Run("creates a user")]`.
- Anonymous `func_literal`s are anchor-only and do not appear in
  the breadcrumb; only the labeled `call_expression` does.
