# Multi-pair replace stays in a single hunk

## Why this case exists

When a `Replace` operation spans multiple adjacent object-literal pairs
(e.g. switching `{ a: OLD_A, b: OLD_B, c: OLD_C }` to a different shape),
the engine used to fragment the change across multiple hunks, splitting
removed lines into one hunk and added lines into another. That made the
diff misleading and dropped some lines entirely.

This case pins the fixed behaviour: every removed line and every added
line in a multi-pair replace appears together in one hunk, anchored on
the enclosing method.

## Behaviours pinned

- All four removed lines (`OLD_A`, `OLD_B`, the two-line `TYPE_C`)
  appear together with all three added lines.
- The import change and the body change collapse into one hunk spanning
  lines 1-13. Because that hunk mixes a top-level import edit with edits
  inside `processTask`, the lowest common ancestor of the changed lines
  is the file root, so the breadcrumb is empty. The
  `class TaskService` / `processTask` headers still appear as context
  lines inside the hunk.

## Notes

The fixtures here mirror a real bug seen on a `task.service.ts` file in
production code. The shapes (`TaskType`, `TaskPriority`, etc.) are
preserved verbatim so any regression looks identical to the historical
report.
