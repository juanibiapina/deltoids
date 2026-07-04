# Merged hunk breadcrumb names the lowest common ancestor

## Why this case exists

When one hunk merges changes at different scope depths, the breadcrumb
must name the **lowest common ancestor** of the changed lines, not the
deepest single chain.

Here a `mod outer` contains a `const LIMIT` (a direct child of the mod,
sibling of the inner `fn run`) and the `fn run` body. Editing both the
`const` and a line inside `fn run` merges the two ranges into one hunk
spanning the whole `mod`. The `const` change is **not** inside `fn run`,
so the breadcrumb must name only `[mod_item outer]`.

## Behaviours pinned

- The two edits merge into a single hunk covering the whole `mod`.
- The breadcrumb is the LCA of the changed lines (`[mod_item outer]`),
  not the inner `[function_item run]`.
