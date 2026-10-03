# Kotlin function body edit

## Why this case exists

Kotlin edits need named function context in the shared diff engine.

## Behaviours pinned

- A body edit expands to the whole compact function.
- The breadcrumb names the enclosing function.
