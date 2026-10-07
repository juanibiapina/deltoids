# Collapsing three functions into one shows each removed line once

## Why this case exists

Three functions collapse into one: the diff deletes from the middle of the
first function to the head of the third, and appends a `const` at the end
of the file. The engine used to split the delete per scope and then let
the end-of-file insert's context overlap it, so `-fn third() {` appeared
in two hunks.

## Behaviours pinned

- One hunk covering the whole (small) file: the delete's context and the
  append's context overlap, so they merge.
- Every removed and added line appears exactly once.
