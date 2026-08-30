# Bare SQL statement change has no breadcrumb

**Why this case exists**

A change in a top-level `SELECT`/`UPDATE` with no surrounding DDL
structure produces no breadcrumb, mirroring other data/statement
scenarios. The line-number box supplies the location.

**Behaviours pinned**

- A change in a bare statement produces no ancestor breadcrumb.
