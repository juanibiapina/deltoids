# Expanding a large function's hunk shows the whole function

## Why this case exists

A change inside a function over 200 lines gets only 100 lines of context
on each side (see case 048). Expanding the hunk once shows the whole
function, which automatic expansion never does for such a large scope.

## Behaviours pinned

- One expansion grows the hunk to the whole function.
- The next expansion grows it to the whole file.

## Notes

The inputs are copies of case 048 with a small function appended, so
the whole function is smaller than the file.
