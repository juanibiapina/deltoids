# TypeScript: wrapping a function body in a call produces a duplicate hunk

## Why this case exists

When a `const` arrow function's body is rewritten to wrap it in a
`withDORetry(() => { … })` call, the diff engine emits two hunks. The
first hunk correctly shows the full file change (comment tweak, new
import, body rewrite). The second hunk is a ghost: it re-emits the
`withDORetry` body as an add-only hunk anchored on
`[call_expression withDORetry]`, duplicating lines that already appear
in the first hunk.

Both `diff -u` and `delta` produce a single hunk for this change.

The root cause is likely the scope engine: the new `call_expression`
node (`withDORetry(…)`) didn't exist in the original file, so the
engine treats its interior as a brand-new scope and emits a separate
hunk for it — even though those added lines are already covered by the
parent `variable_declarator` hunk.

## Behaviours pinned

- The diff should produce **one** hunk covering the whole file, not two.
- The `withDORetry` body must not appear twice.

## Notes

Inputs are the real `apps/api/src/UserDO/stub.ts` from juanibiapina/zero
(commit 75af73f → working tree).
