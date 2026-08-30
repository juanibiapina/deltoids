# SQL `CREATE TABLE` column change carries the table breadcrumb

**Why this case exists**

SQL entered the tree-sitter scope engine so DDL diffs get a breadcrumb.
A column edit inside a `CREATE TABLE` must anchor on the table, and the
breadcrumb name must come from the nested `object_reference` (the
statement node has no `name` field).

**Behaviours pinned**

- A change to a column line breadcrumbs to `[create_table users]`.
- The name is resolved from the `object_reference`, not an empty string.
