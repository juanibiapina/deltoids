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
- The body change lives in a single hunk anchored on
  `[class_declaration TaskService]` `[method_definition mapPriority]`.
- A separate hunk covers the import change at the top of the file.

## Notes

The fixtures here mirror a real bug seen on a `task.service.ts` file in
production code. The shapes (`TaskType`, `TaskPriority`, etc.) are
preserved verbatim so any regression looks identical to the historical
report.
