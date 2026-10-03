# Nested Kotlin script lambda

## Why this case exists

Nested Gradle blocks should retain the inner call as context without swallowing sibling blocks.

## Behaviours pinned

- The inner anonymous lambda anchors the hunk.
- Neither lambda creates a named breadcrumb.
- The sibling task is excluded.
