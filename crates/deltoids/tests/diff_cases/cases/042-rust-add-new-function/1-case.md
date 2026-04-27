# Adding a new top-level function produces a hunk anchored on the new scope

## Why this case exists

When a Rust file gains a brand-new top-level function, the engine
should:

1. Anchor the hunk on the **new** tree (not the old one) so the
   breadcrumb names the function being added.
2. Keep the new function's body together as a single contiguous hunk.
3. **Not** include the unchanged sibling function as context — that
   would make every "add new function" diff drag in the previous
   function for no good reason.

## Behaviours pinned

- One hunk anchored on `[function_item new_function]`.
- The hunk contains only `+` lines (and optionally a leading blank
  line) — no context from `existing()`.
