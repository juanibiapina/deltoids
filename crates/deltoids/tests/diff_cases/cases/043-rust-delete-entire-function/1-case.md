# Deleting an entire function produces a hunk anchored on the deleted scope

## Why this case exists

The mirror of case 042. When a Rust file loses a function, the engine
should:

1. Anchor the hunk on the **old** tree (the new tree no longer has the
   scope) so the breadcrumb names the function being removed.
2. Keep the deleted function together as a single contiguous hunk.
3. **Not** include unchanged neighbour functions as context.

## Behaviours pinned

- One hunk anchored on `[function_item to_delete]`.
- The hunk contains only `-` lines (plus a leading blank line if the
  preceding blank was part of the deletion).
- Bodies of the surrounding `first()` and `third()` functions do not
  appear as context lines.
