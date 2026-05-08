# Terraform / HCL block scope produces a labeled breadcrumb

## Why this case exists

Terraform configs are written as HCL `block`s of the shape
`<type> "<label>" "<label>" { ... }` (`resource "aws_s3_bucket" "logs"`,
`variable "region"`, `module "vpc"`). A change inside one of these
blocks should anchor the hunk on the block and produce a breadcrumb
that names the block by its type plus its labels — not just `[block ]`
with an empty name.

The HCL grammar exposes the block's leading tokens as positional
`identifier` / `string_lit` children with no field names, so the
generic `name`/`property`/`type`/`key` lookup used for other languages
returns nothing. HCL declares `block` as a `positional_name_kinds`
entry; the engine then builds the breadcrumb name by joining those
leading children with spaces.

## Behaviours pinned

- A change inside a Terraform `resource` block produces a hunk
  anchored on the block, with breadcrumb
  `[block resource "aws_s3_bucket" "logs"]`.
- The block's type and string labels are preserved verbatim, including
  the surrounding double quotes.
- Other top-level blocks (`variable`, `module`, …) in the same file
  remain untouched and contribute no context to the hunk.
