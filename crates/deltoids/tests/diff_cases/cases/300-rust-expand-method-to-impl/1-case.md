# Expanding a method hunk grows it to the impl, then the file

## Why this case exists

A change inside a short method shows the whole method. Readers who want
the surrounding type press the expand key: each press grows the hunk to
the next enclosing scope, and the whole file is the last level.

## Behaviours pinned

- The first expansion grows the method hunk to the whole `impl`.
- The second expansion grows it to the whole file.
- A third expansion changes nothing (`(unchanged)`).
- The breadcrumb keeps naming the changed method.
- Shrinking steps back to the `impl`, then to the method, and a
  further shrink changes nothing.
