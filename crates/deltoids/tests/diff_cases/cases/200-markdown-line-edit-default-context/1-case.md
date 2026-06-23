# Markdown line edit uses default context, not the whole document

## Why this case exists

Regression guard for the "expand to a boundary" model when applied to
prose. In Markdown, a heading is a *sibling* of the body beneath it
(both children of a `section`), so a change on a body line has **no
enclosing structure** in its ancestor chain. The raw chain is
`paragraph/list -> section(##) -> section(#) -> document`, and the
outermost non-root ancestor (the `# Title` section) spans the entire
document. Generic transparent expansion would therefore grow a one-line
edit into a hunk covering the whole file.

Markdown sets `transparent_expansion: false`, so `expansion_anchor`
returns `None` for body changes and they fall back to 3-line default
context.

## Behaviours pinned

- A single-line edit inside a Markdown section produces a small hunk
  (the change plus default context), **not** the whole document.
- The breadcrumb is empty (no structure ancestor on a body line).

## Notes

This is the deliberate Option-A compromise: Markdown does not yet get a
section-aware context model, but it must never swallow the file. A
future change will treat sections as boundaries (heading breadcrumbs,
delete/add detection) and can re-enable transparent expansion.
