# Structural Cases

Companion to [`diff_cases/`](../diff_cases/) but for the **structural**
diff layer in `deltoids::structural`. Each case captures the
human-readable change list `format_summary` produces for a given pair
of input files.

## Layout

```text
cases/<NNN-slug>/
  1-case.md                 Title + why this case exists.
  2-original.<EXT>          Pre-edit source.
  3-updated.<EXT>           Post-edit source (matching extension).
  4-expected.structural     Recorded `format_summary` output.
```

The `4-expected.structural` is *exactly* what
`deltoids::structural::format_summary(&StructuralDiff::compute(original,
updated, path))` produces, including the title line and indented
bullet list. An empty file means no structural changes.

## Running

```bash
# Run as integration test
cargo test -p deltoids --test structural_cases

# Refresh every expected file from the current implementation
DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test structural_cases
```

The `DELTOIDS_UPDATE_CASES` env var is the same one used by
`diff_cases`, so a single invocation refreshes both suites.

## Adding a new case

1. Create `cases/<NNN-slug>/`. Match the numeric-prefix convention so
   the directory listing reads in narrative order.
2. Write `1-case.md` (a sentence or two on what behaviour the case
   pins).
3. Write minimal `2-original.<EXT>` and `3-updated.<EXT>` inputs.
4. Run with `DELTOIDS_UPDATE_CASES=1` to generate
   `4-expected.structural`.
5. Read every line of the generated file before committing — this is
   the spec for that scenario.
