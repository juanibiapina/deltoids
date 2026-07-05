# Method rename plus a new wrapper method render faithfully in one hunk

## Why this case exists

When a method is renamed (e.g. `getById` → `fetchById`) alongside a
change inside the method body **and** a brand-new wrapper method is
added nearby, the line-level diff fuses the rename, the body change, and
the new method into `Replace` ops. The diff can match the closing brace
`}` of the old method to a different `}` occurrence in the new file (the
end of the new wrapper).

The builder renders the `Replace` faithfully by op kind, so the rename
(`- getById` / `+ fetchById`), the body change, and the entire new
`getById` wrapper method all render as `+`/`-`/context in one hunk whose
`+/-` counts match `git diff`. No line is dropped and no line is shown
twice, even though the brace cross-matches. The changed lines span two
sibling methods, so the breadcrumb is their shared parent
`[class_declaration ItemService]`.

## Behaviours pinned

- The rename, the body change, and the new wrapper method render in one
  hunk whose `+/-` counts match `git diff`.
- Every added line appears exactly once; the brace cross-match does not
  duplicate or drop any line.
- The breadcrumb names the shared parent `[class_declaration
  ItemService]`, not a single method (the hunk spans two).
