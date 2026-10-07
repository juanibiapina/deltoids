# Expanding every hunk grows each one a level, then the file

## Why this case exists

From the Files sidebar, `z` expands every hunk of the selected file at
once and `x` shrinks every one. Each hunk grows as `Diff::expand` would
grow it alone, even when the growth merges hunks.

## Behaviours pinned

- Two method hunks in one `impl` both grow to the `impl` and merge
  into one hunk.
- The next expansion grows the merged hunk to the whole file, and a
  third changes nothing (`(unchanged)`).
- Both hunks grew to the same `impl`, so the expansion is recorded
  once: shrinking steps back to the `impl`, then splits back into the
  two method hunks, and a further shrink changes nothing.
