# Adding a brand-new top-level public function

## Why this case exists

When a new `pub fn` is added to a Rust source file, the structural
diff should report:

1. Exactly one change of kind `Added`.
2. Visibility annotation `(public)` on the description so public-only
   filters can pick it up.
3. The change is sorted at the position the new symbol takes in the
   updated file.
