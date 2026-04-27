# JSON property change has no breadcrumb

## Why this case exists

JSON is a *data-only* language: it has objects, arrays, and pairs, but
no named functions or classes. The breadcrumb chain renders only
"structure" scopes (functions, classes, modules, …). Data containers
contribute to context expansion (so a hunk in a JSON file can grow to
include the enclosing object) but never appear in the breadcrumb.

## Behaviours pinned

- A change inside a nested JSON object produces a hunk with no
  ancestors.
- The hunk uses scope-expanded context (the change is shown with its
  enclosing object), not just the default 3 lines.
