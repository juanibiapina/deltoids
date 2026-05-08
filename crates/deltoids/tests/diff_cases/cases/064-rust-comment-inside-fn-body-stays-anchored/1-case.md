# Comment edit inside a fn body keeps that fn as anchor

## Why this case exists

The fix for "leading comments attach to the next sibling structure"
(cases `061`/`062`/`063` and friends) must NOT promote a comment that
is **inside** a function body. An in-body comment is a child of the
body, not a sibling of a structure; the enclosing fn is already the
correct anchor and the helper must be a no-op there.

This case is the regression guard: the only edit is a body-internal
comment, and the hunk must stay anchored on the surrounding fn — never
on the next-sibling fn.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[function_item outer]`.
- Hunk does not include `fn next`.
