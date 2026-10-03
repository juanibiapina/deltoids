# Kotlin script trailing lambda edit

## Why this case exists

Gradle Kotlin DSL changes need the enclosing block as context without invented breadcrumb names.

## Behaviours pinned

- A `.kts` file uses Kotlin scope extraction.
- The trailing lambda anchors context at the call opening line.
- Anonymous lambdas add no named breadcrumb or unrelated repository block.
