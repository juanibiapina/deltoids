# Replace additions dropped in second hunk

## Why this case exists

When a diff has two hunks — first a pure insertion, second a replace
(delete + add) inside a nested arrow function — the engine drops the
added lines of the second hunk, showing only the deletions. The root
cause is in scope/hunk_builder: the added lines of a `Replace` op are
not emitted when building the hunk.

Reproduces the bug seen in `juanibiapina/zero`
`apps/api/src/routes/telegram-webhook.ts`.

## Behaviours pinned

- The second hunk contains both the deleted (`-`) and added (`+`) lines
  of the replace.
- The first hunk (pure insertion) is unaffected.
