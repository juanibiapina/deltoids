# CSS `/* … */` comment-only edit above a rule anchors on the rule

## Why this case exists

A CSS `/* … */` comment above a `rule_set` is a sibling of the
rule_set inside the stylesheet. A comment-only edit today finds no
enclosing structure (rule_sets sit at the top level) and falls back to
default 3-line context with no breadcrumb.

CSS rule names are derived from the selector via the `name` field
fallback chain (selectors don't have a `name` field in tree-sitter-css,
so the breadcrumb name will be the empty string for now). The
behaviour we pin here is the *anchor* — the comment attaches to the
following rule even when the rule's printed name is empty.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors include `[rule_set ]` (or whatever name is rendered for
  the rule).
- Hunk does not include sibling rules.
