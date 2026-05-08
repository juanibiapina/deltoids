# Terraform resource block: breadcrumb + multi-line literal context

Edits inside a Terraform `block` (`resource "aws_s3_bucket" "logs"`,
`variable "region"`, `module "vpc"`, …) anchor the hunk on the block
and produce a breadcrumb that names it by its type plus its positional
string labels. When the change lands inside a multi-line `tuple` /
`object` literal nested in the block, the hunk also covers the whole
literal so the binding line and the surrounding entries stay visible.

## Why this case exists

`.tf` files are detected as HCL with `structure_kinds: ["block"]`, but
tree-sitter-hcl's `block` has no `name` field — its identity sits in
positional `identifier` / `string_lit` children. Without a positional
naming path the breadcrumb came out as `[block ]`. A per-language
`positional_name_kinds` knob (set to `["block"]` for HCL) routes those
kinds through a `positional_name` helper that joins leading
`identifier` / `string_lit` named children with spaces, stopping at the
first other kind so the body doesn't leak into the name.

HCL also gets `data_kinds: ["tuple", "object"]` so that multi-line
collection literals — the `routes = [ {…}, {…} ]` shape that pervades
real Terraform — provide a data-tier fallback. When an enclosing block
exceeds `MAX_SCOPE_LINES`, the planner can still expand the hunk to
the surrounding literal instead of dropping to the 3-line default.
This case pins the in-block shape; the data fallback path also kicks
in for large blocks where the block itself can't be expanded.

## Behaviours pinned

- A change inside a `resource "TYPE" "NAME"` block produces a
  `[block resource "TYPE" "NAME"]` breadcrumb.
- The block's positional labels appear in the order they're written
  in source.
- A change inside a multi-line `tuple` of `object` literals nested in
  the block keeps the binding line (`routes = [`) and the surrounding
  object entries visible as context.
