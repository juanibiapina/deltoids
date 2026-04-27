# Single line replaced in plain text

## Why this case exists

Plain text replacement establishes the baseline shape of a `Replace`
operation in the unified diff fallback path: matching `-`/`+` lines
appear adjacent in the same hunk.

## Behaviours pinned

- One hunk, no ancestors.
- The replaced line appears as `-old` immediately followed by `+new`.
- Surrounding context lines are included with the standard 3-line
  radius.
