# Body-only edit on a private function

## Why this case exists

When the body of an existing function changes but the signature and
visibility stay the same, the change should be reported as
`BodyChanged` with the human description "Modified function `helper`",
without a `(public)` suffix.

This is the common case for refactors: most of the diff is body-only.
