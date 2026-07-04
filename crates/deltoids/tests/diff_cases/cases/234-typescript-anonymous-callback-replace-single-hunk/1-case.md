# TypeScript: an anonymous callback in a Replace stays in one hunk

## Why this case exists

Regression guard for the labeled-vs-unlabeled distinction. Rewriting a
top-level expression to expand an anonymous `(x) => …` callback into a
block body produces a `Replace` whose new content contains an anonymous
`arrow_function`. It is **not** a labeled callback (its enclosing
`compute(items, …)` call's first argument is `items`, not a string
label) and the `total` declarator's value is a call (not an arrow), so
there is no named scope. New-scope detection must therefore emit **no**
separate hunk: the whole rewrite stays in one hunk instead of spawning a
ghost hunk for the anonymous callback.

## Behaviours pinned

- The expression rewrite renders as a single hunk.
- No add-only "new scope" hunk is produced for the anonymous callback.
- The hunk has no breadcrumb: the rewrite is a top-level `const`
  statement, so the lowest common ancestor of the changed lines is the
  file root.
