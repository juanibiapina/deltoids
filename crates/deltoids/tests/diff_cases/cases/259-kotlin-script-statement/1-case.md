# Kotlin script statement edit

## Why this case exists

A script statement without a named declaration must not invent a scope name.

## Behaviours pinned

- A top-level single-line script edit uses default context.
- No breadcrumb is emitted.
