# Adding a comment above a documented TOML table shows the comment

## Why this case exists

A new `#` line is inserted above the existing comment that documents
`[other]`. The engine anchored the insert on the `[other]` table, whose
bounds start below the insertion point, and the hunk came out empty: the
diff showed nothing at all.

## Behaviours pinned

- One hunk with the added comment, anchored on the `[other]` table.
- The hunk's context grows to include the insertion point.
