# YAML property change has no breadcrumb

## Why this case exists

YAML — like JSON — is a data-only language for the diff engine. A
change deep in a nested mapping should produce a hunk that uses
scope-expanded context (so the surrounding mapping is visible) but
**without** an ancestor breadcrumb, since YAML mappings have no name
the engine can show.

## Behaviours pinned

- A change inside a nested YAML mapping produces a hunk with no
  ancestors.
- The hunk uses scope-expanded context (the enclosing block is
  visible), not the default 3-line radius.
