# Large function falls back to 3-line default context

## Why this case exists

Scope expansion would dump too much code on screen if every change
inside a 1000-line function expanded to the whole function. The engine
caps scope expansion at `MAX_SCOPE_LINES` (currently 200). Beyond that,
the hunk falls back to standard 3-line context, even though the
ancestor breadcrumb still names the enclosing function.

## Behaviours pinned

- A change inside a function with > `MAX_SCOPE_LINES` body uses
  default 3-line context (not full-function expansion).
- The breadcrumb still names the enclosing function — the cap only
  affects how much surrounding code we include, not where we anchor
  the hunk.

## Notes

The `2-original.rs` / `3-updated.rs` files are deliberately long (over
200 body lines). Skim the start/end to confirm the scope; the change
itself is a single line near the middle.
