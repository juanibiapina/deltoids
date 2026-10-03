# Kotlin callback inside a function

## Why this case exists

Callbacks inside a named function follow the engine's local-helper demotion rules.

## Behaviours pinned

- Context retains the whole enclosing function.
- The breadcrumb names the outer function and omits the anonymous lambda.
