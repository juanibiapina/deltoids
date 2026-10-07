# Expanding one hunk merges the hunks it now overlaps

## Why this case exists

Two methods of one `impl` change, so the diff shows two hunks. Expanding
the first hunk to the `impl` covers the second change too. The two hunks
must merge into one so every changed line still appears exactly once.

## Behaviours pinned

- Before expanding, the two method edits are separate hunks.
- Expanding hunk 0 yields one hunk covering the whole `impl` with both
  changes.
- The merged hunk's breadcrumb is the common ancestor of its changes
  (the `impl`).
- Shrinking the merged hunk splits it back into the two method hunks.
