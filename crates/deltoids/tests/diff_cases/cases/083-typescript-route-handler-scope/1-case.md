# Change inside a labeled route handler callback

## Why this case exists

Express-style routers register handlers with an inline arrow
function: `app.get("/users", (req, res) => { ... })`. The arrow
body is the unit of behaviour, but the arrow has no syntactic name;
the identity of the handler sits on the surrounding
`call_expression` (the callee `app.get` plus the route label).

This case mirrors case 082 but exercises a non-test pattern, to
confirm the fix is about labeled callbacks in general (not a
hard-coded test-framework rule).

## Behaviours pinned

- The hunk anchors on the route handler's arrow-function body, not
  on the inner `const filters = { ... }` literal.
- The hunk's breadcrumb is
  `[call_expression app.get("/users")]`.
- The arrow function itself is anchor-only and does not appear in
  the breadcrumb.
