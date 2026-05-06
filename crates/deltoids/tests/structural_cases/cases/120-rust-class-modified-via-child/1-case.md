# Adding a method to an impl drops the impl from the change list

## Why this case exists

When a struct/trait/impl/class is "modified" only because one of its
children (a method) changed, the container change is redundant — the
child change already tells the reader everything they need to know.
The structural diff suppresses these container-level entries when a
strictly-deeper sibling change is present.

This keeps the summary focused on leaf-level changes and matches what
reviewers expect: "the new method was added", not "the type changed
(and also the new method was added)".
