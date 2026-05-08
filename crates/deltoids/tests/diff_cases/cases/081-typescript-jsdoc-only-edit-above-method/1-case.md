# JSDoc-only edit above a TS class method anchors on the method

## Why this case exists

In TypeScript, a `/** … */` JSDoc comment above a class method is
parsed as a `comment` sibling of `method_definition`, not a child.
Today an edit on the JSDoc anchors on `class_declaration` and the hunk
expands to cover unrelated sibling methods.

## Behaviours pinned

- One hunk for the JSDoc edit.
- Ancestors: `[class_declaration Foo] [method_definition bar]`.
- Hunk does not include sibling methods `other` / `another`.
