# A field added just before a new impl keeps its struct as context

## Why this case exists

A struct gains a field and a new `impl` block is added right after it.
The line diff (like `git diff`) matches the struct's old closing `}`
against the impl's closing `}`, so one insert carries the new field,
the struct's `}`, a blank line, and the impl minus its last line.

That insert starts inside an existing scope, so it is an edit to the
struct, not a clean insertion of a new scope. The engine used to see
`fn lines` lying wholly inside the insert and treat the whole block as a
new scope: no context, and a breadcrumb naming `impl › fn lines` above a
struct field. The struct header and existing fields were missing, so the
new field had no visible owner.

## Behaviours pinned

- One hunk whose context starts at the struct header and shows the
  existing fields above the new one.
- The added lines match `git diff` exactly: the field, the struct's
  `}`, the blank line, and the impl up to its body; the impl's closing
  `}` is context.
- The breadcrumb names the struct. The added lines span the struct and
  the new impl, so their lowest common ancestor is the file root, and the
  breadcrumb falls back to the scope the hunk is anchored on.

## Notes

Cases 042 and 055 still own clean inserts of whole new scopes, which
keep their own hunk anchored on the new scope.
