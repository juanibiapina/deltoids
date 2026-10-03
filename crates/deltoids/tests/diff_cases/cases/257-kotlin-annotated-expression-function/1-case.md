# Annotated Kotlin extension function

## Why this case exists

Expression bodies, annotations, and backtick names must retain declaration identity.

## Behaviours pinned

- An expression-body edit keeps leading documentation and annotation as context.
- The extension receiver is not mistaken for the function name.
- The breadcrumb preserves the backtick identifier.
