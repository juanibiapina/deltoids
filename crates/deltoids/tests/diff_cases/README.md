# Diff Cases

This directory is a **product reference** for the deltoids diff engine.
Each subdirectory under `cases/` is one scenario, laid out so a plain
directory listing reads in narrative order:

1. `1-case.md` — the explainer.
2. `2-original.<EXT>` — the input.
3. `3-updated.<EXT>` — the modified input.
4. `4-expected.diff` — the diff we expect to produce.

`original` / `updated` match the parameters of `Diff::compute(original,
updated, …)`. The numeric prefixes only exist to make the listing
ordered.

The same files double as an integration test. The harness in
[`harness.rs`](./harness.rs) walks every case, runs `Diff::compute`, and
asserts the result matches the recorded expectation. Every case, and a
seeded sweep of random edits over the case inputs, must also pass the
exact-cover check in [`exact_cover.rs`](./exact_cover.rs): each changed
line appears in exactly one hunk, hunks are ordered and disjoint, and
no hunk is context only.

## Why this exists

When we change anything in the diff pipeline (scope detection, hunk
merging, intra-line emphasis, …) we want to know exactly which scenarios
move and how. The cases here are that catalogue.

* **For users / readers**: open any case, read `1-case.md`, look at
  `2-original.<EXT>`, `3-updated.<EXT>`, then read `4-expected.diff`.
  You can see exactly what the engine produces and why.
* **For tests**: the `diff_cases` integration test refuses to pass
  unless every recorded `4-expected.diff` is reproduced exactly.
* **For new features / bug fixes**: every behaviour change starts as a
  new case here. The case describes the scenario and pins the desired
  output. The test then drags the implementation to the spec.

## Layout

```text
cases/<NNN-slug>/
  1-case.md          Title, why this case exists, behaviours pinned,
                     manual review notes.
  2-original.<EXT>   File content before the edit. The extension picks
                     the language for tree-sitter (`.rs`, `.ts`,
                     `.json`, …).
  3-updated.<EXT>    File content after the edit. Must use the same EXT.
  4-expected.diff    Recorded `Diff::compute` output (case format below).
  5-expand.txt       Optional: hunk indices passed in order to
                     `Diff::expand` (`Diff::shrink` when prefixed
                     with `-`), one per line. `*` calls
                     `Diff::expand_all` and `-*` `Diff::shrink_all`.
  6-expanded.diff    Recorded output after each expansion; required
                     with `5-expand.txt`.
```

A directory whose name starts with `_` or `.` is skipped. Names are
sorted lexicographically; numbered prefixes (`010-`, `020-`, …) keep
related cases grouped.

## Case format (`expected.diff`)

The file looks like a unified diff with one extension: the line after
`@@` carries the hunk's ancestor breadcrumb chain. Each ancestor is
written as `[KIND name]`, outermost first, separated by spaces. When a
hunk has no ancestors the breadcrumb section is empty.

```text
@@ -1,5 +1,5 @@ [impl_item Foo] [function_item compute]
 fn compute(&self) -> i32 {
     let x = 1;
-    x + 1
+    x + 2
 }
```

* Single-line ranges drop the `,COUNT` (matches `git diff` style):
  `@@ -7 +7 @@` instead of `@@ -7,1 +7,1 @@`.
* Multiple hunks are separated by a blank line.
* A diff that produces no hunks (identical files) is the empty string.

`6-expanded.diff` holds one section per line of `5-expand.txt`: a
`## expand N` or `## shrink N` line (`all` for `*` steps), then the
hunks after that step on the previous result, or `(unchanged)` when
there is no larger level.

## Running the cases

```bash
# Run all cases as integration tests
cargo test -p deltoids --test diff_cases

# Refresh every expected.diff from the current implementation.
# Use this when adding a new case, then review the generated files.
DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test diff_cases
```

Failures print a unified diff between the recorded `4-expected.diff`
and the current actual output, plus the path to the case directory.

## Adding a new case

1. Pick a unique slug. Use the next free three-digit prefix in the
   theme group (e.g. `045-rust-private-fn`).
2. Create `cases/<NNN-slug>/`.
3. Write `2-original.<EXT>` and `3-updated.<EXT>` (matching
   extensions). Keep them as small as possible while still triggering
   the behaviour.
4. Write `1-case.md` with:
   * `# <Title>` (h1) summarising the scenario.
   * **Why this case exists** — the bug or feature it pins down.
   * **Behaviours pinned** — bullet list of what the case asserts.
   * Optional "Notes" section for manual review tips.
5. Run the suite in update mode to generate `4-expected.diff`:
   ```bash
   DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test diff_cases
   ```
6. Review the generated `4-expected.diff` by hand. If it matches what
   you intend, commit. If not, fix the implementation, the case
   inputs, or the description until both line up.

## Index of cases

Cases are organised loosely by theme via their numeric prefix:

* `010-019` — degenerate inputs (empty, identical, …)
* `020-039` — plain-text scenarios (no language support)
* `040-069` — Rust scope behaviour
* `070-079` — language-as-data files (JSON, TS configs)
* `080-089` — TypeScript / JavaScript class & method scopes
* `090-099` — YAML and other config-shaped languages
* `100-109` — Python scope behaviour
* `110-119` — Go scope behaviour
* `120-129` — Ruby scope behaviour
* `130-139` — C scope behaviour
* `140-149` — C++ scope behaviour
* `150-159` — Lua scope behaviour
* `160-169` — HCL / Terraform scope behaviour
* `170-179` — Java scope behaviour
* `180-189` — Bash scope behaviour
* `190-199` — CSS scope behaviour
* `200-209` — Markdown scope behaviour
* `220-229` — TOML scope behaviour
* `230-239` — replaces that add or split scopes
* `240-249` — SQL scope behaviour
* `250-259` — Kotlin scope behaviour
* `300-309` — manual expansion (`Diff::expand`)

Each case's `1-case.md` describes what it pins.
