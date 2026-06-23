# Markdown paragraph rewrite uses default context, not the whole document

## Why this case exists

Companion to `200`, for a multi-line change. Rewriting a whole
paragraph inside a section is still a body change with no enclosing
structure, so it must use 3-line default context rather than expanding
to the entire `# Title` section (the whole document).

## Behaviours pinned

- A multi-line paragraph rewrite inside a Markdown section produces a
  hunk bounded by the change plus default context, **not** the whole
  document.
- The breadcrumb is empty (no structure ancestor on body lines).
