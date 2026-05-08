# Change inside a busted `it(...)` callback

## Why this case exists

Lua's busted test framework uses
`describe("Subject", function() it("name", function() ... end) end)`,
the same labeled-callback pattern as Jest. The `it` call's
anonymous `function_definition` is the unit of behaviour, but the
function has no syntactic name; the identity of the test sits on
the surrounding `function_call` (the callee `it` plus its
string-literal first argument).

Today, when a change sits inside a table constructor inside an `it`
callback, the engine anchors only on the inner `table_constructor`
data scope and the breadcrumb is empty — the reader cannot tell
which test is being changed.

This is the Lua equivalent of case 082 (TypeScript Jest).

## Behaviours pinned

- The hunk anchors on the inner `it` callback's `function_definition`
  body, not on the inner `scope = { ... }` table.
- The hunk's breadcrumb is
  `[function_call describe("UserService")]`
  `[function_call it("creates a user")]`.
- Anonymous Lua `function_definition`s are anchor-only and do not
  appear in the breadcrumb; only the labeled `function_call` does.
