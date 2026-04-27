# Identical files produce no hunks

## Why this case exists

`Diff::compute` must short-circuit cleanly when there is nothing to
report. A spurious hunk on identical input would lead to noisy "empty"
hunks rendering on screen and would imply the engine considers the files
different.

## Behaviours pinned

- Identical `before` and `after` produce zero hunks.
- The case-format serialisation of zero hunks is the empty string.

## Notes

The file extension is `.rs` here only because the harness needs *some*
extension to drive language selection. The behaviour is the same for any
language and for unsupported extensions.
