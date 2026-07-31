# Comment-only edit above an exported arrow function anchors on the function

## Why this case exists

Mirror of `091` for the `export const f = () => {}` shape. Editing only
the doc comment above the declaration must anchor the hunk on the
function and carry its breadcrumb, the same way a comment-only edit
above a plain `function` declaration does.

The comment's following sibling is the `export_statement`, not the
scope: the scope is the promoted `variable_declarator` nested inside it.
Resolving the start node has to descend into the wrapper, otherwise the
walk only sees ancestors of the `export_statement` and the hunk ends up
with no breadcrumb and default context.

## Behaviours pinned

- One hunk covers the comment edit.
- The breadcrumb names the arrow-function scope.
- The hunk context runs to the end of the function, not three default
  lines.
