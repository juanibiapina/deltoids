# Removing a private struct from a Rust file

## Why this case exists

A removed symbol should be reported as `Removed` with no `(public)`
suffix. Private removals still appear in the default summary; they are
hidden only when the public-only filter is on.
