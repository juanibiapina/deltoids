# Expanding a Markdown hunk grows it section by section

## Why this case exists

Markdown changes get 3 lines of context because whole sections are too
large to show by default. Expanding walks up the heading structure
instead of jumping to the whole document.

## Behaviours pinned

- The default context already holds the `### Linux` subsection, so the
  first expansion shows the `## Install` section with both subsections.
- The next expansion shows the whole document.
- A further expansion changes nothing.
