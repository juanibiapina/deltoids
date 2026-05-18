# No overlapping hunks when insert sits beside object literal in large function

## Why this case exists

When a function exceeds MAX_SCOPE_LINES (200), changes fall back to
default 3-line context. An insert just before an object literal and a
replace inside that same object create two context ranges with
**different** `scope_id`s: the insert gets the function's id (no data
scope at its line), and the replace gets the object literal's id.
The different ids prevent merging, causing the ranges to overlap and
the same removed/added lines to appear in **two** hunks.

This is the root cause of the "content shown twice" bug when reviewing
diffs of large functions.

## Behaviours pinned

- An insert of a comment line and a replace inside the adjacent object
  literal produce a **single** merged hunk, not two overlapping hunks.
- No line appears in more than one hunk.
- The hunk's breadcrumb anchors on `lambdaHandler`.
