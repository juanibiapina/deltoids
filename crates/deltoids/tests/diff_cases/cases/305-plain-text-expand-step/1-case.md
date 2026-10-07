# Expanding a hunk in a file with no parser adds 20 lines per side

## Why this case exists

Files with no tree-sitter grammar have no scopes to grow into. Each
expansion adds 20 lines of context on each side, clamped to the file.

## Behaviours pinned

- The first expansion grows 3 lines of context to 23 on each side.
- The second expansion reaches the file's start and end.
- A third expansion changes nothing.
