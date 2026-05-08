# Java Javadoc-only edit above a method anchors on the method

## Why this case exists

A Java `/** … */` Javadoc above a method is a `block_comment` sibling
of `method_declaration`, child of `class_body`. A comment-only edit
today anchors on `class_declaration` and the hunk covers sibling
methods.

## Behaviours pinned

- One hunk for the Javadoc edit.
- Ancestors: `[class_declaration Foo] [method_declaration bar]`.
- Hunk does not include sibling methods.
