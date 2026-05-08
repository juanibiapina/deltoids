# HCL/Terraform `#` comment-only edit above a block anchors on the block

## Why this case exists

A `#` or `//` comment above a Terraform `resource`/`variable`/`module`
block is a sibling of the `block` node. A comment-only edit at the
file level today finds no enclosing structure and falls back to
default 3-line context with an empty breadcrumb.

## Behaviours pinned

- One hunk for the comment edit.
- Ancestors: `[block resource "aws_s3_bucket" "logs"]`.
- Hunk does not include sibling blocks.
