# Large function uses bounded context instead of full expansion

## Why this case exists

Scope expansion would dump too much code on screen if every change
inside a 1000-line function expanded to the whole function. The engine
caps full-scope expansion at `MAX_SCOPE_LINES` (currently 200). Beyond
that, the hunk uses a `STRUCTURE_CONTEXT` budget (100 lines before and
100 lines after the change), clamped to the structure's boundaries.
The ancestor breadcrumb still names the enclosing function.

## Behaviours pinned

- A change inside a function with > `MAX_SCOPE_LINES` body uses
  100-line context per side (not full-function expansion).
- The breadcrumb still names the enclosing function.
- The hunk does not extend past the function boundaries.

## Notes

The `2-original.rs` / `3-updated.rs` files are deliberately long (over
200 body lines). The change is a single line near the middle; context
expands ~100 lines in each direction.
