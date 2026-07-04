# Method rename stays in one hunk; a new wrapper method gets its own

## Why this case exists

When a method is renamed (e.g. `getById` → `fetchById`) alongside a
change inside the method body **and** a brand-new wrapper method is
added nearby, the line-level diff fuses the rename, the body change,
and the new method into `Replace` ops. Two behaviours must hold at
once:

- The rename (`- getById` / `+ fetchById`) and the body change must
  stay in **one** hunk. The diff can match the closing brace `}` of the
  old method to a different `}` occurrence in the new file (the end of
  the new wrapper), which makes the naive end-line `same_slot` check
  fail; the planner's interior-line same-slot probe keeps the rewritten
  member recognised as the same slot so it does not split.
- The brand-new `getById` wrapper method is a fresh named scope, so it
  gets its **own** add-only hunk with its own breadcrumb, rather than
  being lumped into the rename hunk.

## Behaviours pinned

- The method rename (`- getById` / `+ fetchById`) and the body change
  appear together in one hunk.
- The new wrapper method renders as a separate hunk containing all of
  its lines (no dropped lines, no duplication).
- Each hunk's breadcrumb names the method via the class scope.
