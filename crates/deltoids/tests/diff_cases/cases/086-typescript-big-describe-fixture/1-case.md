# Change inside a describe-level fixture in a long describe block

## Why this case exists

Real Jest/Vitest test files commonly declare a fixture object at the
top of `describe(...)` and then run dozens of `it(...)` cases,
easily pushing the describe's arrow body past the engine's
`MAX_SCOPE_LINES` cap. When a change happens inside the fixture
object, the inner arrow-only anchor doesn't fit, so the engine
falls back to the inner `object` data scope. With no breadcrumb on
top, the reader sees just the fixture literal and cannot tell
which test suite is being changed.

This case pins the requirement that **even when the enclosing
arrow callback is too large to size the hunk, the breadcrumb still
identifies the surrounding `describe(...)` call** so the change
locates itself in the suite. The fix relies on promoting
`call_expression` to a named structure (callee + first
string-literal arg) — the call-site identity survives even when
the inner arrow body is too big to anchor on.

## Behaviours pinned

- When the describe block exceeds `MAX_SCOPE_LINES`, the hunk uses
  100-line context per side (clamped to the describe block boundaries)
  instead of falling back to the small data-scope object literal.
- The hunk's breadcrumb is
  `[call_expression describe("BigService")]`, naming the suite
  even though the body that contains the change doesn't fit in full.
