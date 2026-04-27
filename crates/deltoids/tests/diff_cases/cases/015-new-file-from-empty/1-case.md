# New file: empty original, scope detection skipped

## Why this case exists

When the original file is empty (a brand-new file), running tree-sitter
on the "after" content and reporting ancestor scopes for every added
line would produce misleading breadcrumbs — the entire file *is* the
change. The engine must short-circuit and emit a single hunk with no
ancestors.

## Behaviours pinned

- New files (empty `before`) produce exactly one hunk.
- That hunk has no ancestor breadcrumb.
- Every line of the new file appears in the hunk as `+`.
