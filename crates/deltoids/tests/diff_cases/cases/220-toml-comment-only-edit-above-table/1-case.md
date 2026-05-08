# TOML `#` comment-only edit above a table anchors on the table

## Why this case exists

A TOML `#` comment above a `[section]` is a `comment` sibling of the
following `table` node. A comment-only edit today finds no enclosing
structure (tables sit at the top level) and falls back to default
3-line context with no breadcrumb.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[table prod]` (or however the breadcrumb name is built
  for `[database.prod]` — current rule joins dotted_key parts via the
  `name` field).
- Hunk does not include sibling tables.
