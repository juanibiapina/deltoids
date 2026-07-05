# Rust: a Replace that adds multiple new functions renders faithfully

## Why this case exists

The Rust analogue of cases 230/231, mirroring the wild trigger on
`crates/deltoids-cli/src/sidebar/icons.rs`: an edit changed one
function's body **and** appended two new `#[test]` functions. The
line-level diff fuses the edit and the appends into a single `Replace`:

```
Equal   { old_index: 0, new_index: 0, len: 3 }
Replace { old_index: 3, old_len: 1, new_index: 3, new_len: 11 }
Equal   { old_index: 4, new_index: 14, len: 2 }
```

The builder renders every line of the `Replace` by its op kind, so the
displayed `+/-` counts match `git diff` exactly. The whole edit renders
as one hunk: the body change, the two new functions (including the blank
separator lines), and the closing `}` of the last new function as
context (the line engine matched it against the original `}`, exactly
like git).

This pins the fix for a regression where a cohesion feature rendered new
functions by their tree-sitter span, which dropped the blank separators
and re-labeled the matched closing `}` as an addition, so summed hunk
line counts diverged from git.

## Behaviours pinned

- The edit and both new functions render in one hunk whose `+/-` counts
  match `git diff` (+11 -1).
- Blank separator lines between the new functions are shown as `+`, not
  dropped.
- The last function's closing `}` renders as context (matched), not as
  an addition.
- The changed lines span several sibling functions, so the breadcrumb is
  their shared parent `[mod_item tests]`.
