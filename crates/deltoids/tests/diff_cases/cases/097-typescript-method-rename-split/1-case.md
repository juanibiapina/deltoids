# Method rename stays in a single hunk

## Why this case exists

When a method is renamed (e.g. `getById` → `fetchById`) alongside a
change inside the method body and a new wrapper method added nearby,
the diff algorithm can match the closing brace `}` of the old method
to a different `}` occurrence in the new file (e.g. the end of the
wrapper). This makes `same_slot` fail because the end-line mapping
points past the renamed method, causing `new_replace_scope_range` to
create a separate `prevent_merge` range for the new signature — and
the rename splits across two hunks.

## Behaviours pinned

- The method rename (`- getById` / `+ fetchById`) and the body change
  appear together in **one** hunk, not two.
- The hunk breadcrumb uses the old method name (`getById`), identifying
  which method was changed.
