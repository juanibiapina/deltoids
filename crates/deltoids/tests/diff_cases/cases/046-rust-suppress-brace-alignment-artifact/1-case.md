# Closing-brace alignment artifacts do not leak into the diff

## Why this case exists

When a single fn is deleted just below an in-place edit, `similar` is
free to align the deleted fn's closing `}` with the surviving fn's
closing `}`. Both choices have the same edit cost, but the alignment
it picks marks the **survivor's** `}` as removed and the **deleted
fn's** `}` as kept. The visible diff then claims the survivor lost
its closing brace, and the deleted hunk silently misses one line.

The engine should normalize that boundary: any `}` (or other line)
that is identical between the start of a `Delete`/`Insert` op and the
start of the following `Equal` op should land on the kept side, so
the structurally-deleted brace ends up inside the deletion hunk where
it belongs.

## Behaviours pinned

- The surviving fn's hunk closes with ` }` as a context line; no
  trailing `-}`.
- The deleted fn's hunk includes the deleted `fn` block in full,
  closing brace and all. The trailing blank line either side of the
  deleted block also belongs to the deletion hunk; the exact slider
  position (leading vs. trailing blank) is left to the engine and
  matches git's `--diff-algorithm=histogram` default.
- Both hunks anchor on the right scope (`first` / `second`).
