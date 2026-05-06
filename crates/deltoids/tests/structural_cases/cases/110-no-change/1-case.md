# Identical files produce no structural changes

## Why this case exists

When `original == updated`, `format_summary` returns the empty string
(no title, no bullet list). The expected file is empty. This guards
against accidental false-positive change detection.
