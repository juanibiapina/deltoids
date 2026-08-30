# SQL `CREATE FUNCTION` body change carries the function breadcrumb

**Why this case exists**

A change inside a `CREATE FUNCTION` body must anchor on the function, so
the breadcrumb reads `[create_function add]`. The name comes from the
nested `object_reference`.

**Behaviours pinned**

- A change inside the function body breadcrumbs to `[create_function add]`.
