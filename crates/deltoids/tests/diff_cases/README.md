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
asserts the result matches the recorded expectation.

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

Current cases:

| Slug                                                | What it pins                                                              |
| --------------------------------------------------- | ------------------------------------------------------------------------- |
| `010-identical-files`                               | Identical input → no hunks                                                |
| `015-new-file-from-empty`                           | Empty `original` → single hunk, no breadcrumb                             |
| `020-deleted-file-to-empty`                         | Empty `updated` → single hunk, no breadcrumb                              |
| `025-plain-text-line-added`                         | Plain-text append produces one `+` line                                   |
| `030-plain-text-line-replaced`                      | Plain-text replace produces adjacent `-`/`+` lines                        |
| `035-unsupported-language-extension`                | Unknown extension falls back to plain unified diff                        |
| `040-rust-line-in-function`                         | Hunk inside Rust `fn` carries the function as ancestor                    |
| `042-rust-add-new-function`                         | Adding a new top-level fn anchors the hunk on the new scope               |
| `043-rust-delete-entire-function`                   | Deleting a fn anchors the hunk on the deleted scope                       |
| `045-rust-nested-impl-method`                       | `impl` + `fn` produces a two-level breadcrumb                             |
| `048-rust-large-function-falls-back-to-default-context` | Bodies > `MAX_SCOPE_LINES` use 3-line context with full breadcrumb    |
| `050-rust-top-level-statement`                      | Top-level statement → no breadcrumb                                       |
| `055-rust-add-helper-no-duplication`                | New helper appears in exactly one hunk, not duplicated as context         |
| `060-rust-comment-anchor-inside-fn`                 | Doc-comment edit above a fn keeps the fn as ancestor                      |
| `070-json-property-change`                          | JSON change → no breadcrumb (data-only language)                          |
| `075-typescript-config-property-change`             | TS config object literal → no breadcrumb                                  |
| `080-typescript-method-modification`                | Class method change → `[class_declaration X] [method_definition Y]`       |
| `085-typescript-multi-pair-replace`                 | Multi-pair `Replace` stays in a single hunk                               |
| `090-yaml-property-change`                          | YAML change → no breadcrumb, scope-expanded context                       |
