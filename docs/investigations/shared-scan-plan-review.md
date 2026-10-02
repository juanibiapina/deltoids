# Shared-scan plan verification

## Verdict

The plan review supported implementation with the corrections below. At review time, a standalone prototype matched patch/status behavior and reduced repository query time without changing production code. The implementation is now complete; actual TUI measurements and platform checks are recorded in `background-cpu.md` under **Implemented shared repository snapshot**. This report retains the original review measurements.

Reviewed checkout: `0de7f7f`, fetched and equal to `origin/main`. Plan: **Share Git refresh scans**, session attachment ID `b2f49315a84365f8b34c69c7`.

## Proof

At review time, `shared-scan-plan-probe.rs` included the checkout's actual `crates/deltoids/src/git.rs` as its reference implementation. The current probe links the compiled library and checks the implemented snapshot in addition to the original prototype. Its candidate captures HEAD/index, creates one index-to-workdir diff, merges that into a patch diff, prints the patch, and then derives per-column status from the retained workdir diff and a second HEAD-to-index diff. The review introduced no production implementation.

`shared-scan-plan-fixtures.py` creates real temporary repositories. The final run passed **30 cases**, with **one unsupported filename case skipped**. Cases cover clean/unborn/detached HEAD, each change column, staged/workdir cancellation, untracked/ignored files, deletions/recreation, staged additions later deleted, renames on each side, edited/chained/case-only renames, rename configuration, type changes, binary files, conflicts, filters, submodules, 1,000 changed files, staging-only refresh through retained handles, and damaged HEAD rejection.

For the ordinary parity cases the prototype patch equals the current patch byte-for-byte, and sorted staging records equal the current status result. It also checks that merging, rename detection, and printing the net patch leave the retained workdir diff's recorded paths, hashes, status, and flags unchanged. The faulty-HEAD case requires both the existing status reader and the corrected prototype to reject the fixture. The retained-handle case verifies external staging/unstaging, equal net patch text, changed staging labels, and unchanged index bytes/mtime after snapshot reads.

Final fixture output: `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-shared-scan-review-b11nn_cu/results.json`.

### Query measurements

Optimized Rust probe, native libgit2 **1.9.7**, twenty warm iterations per method on the original approximately 30,000-file temporary fixture. The reference is the actual current source compiled into the same executable as the prototype.

| Index refresh policy | Current patch + status | Shared snapshot | Reduction |
| --- | ---: | ---: | ---: |
| Conditional index reload, `read(false)` | 175.851 ms | 95.603 ms | 45.63% |
| Forced index reload, `read(true)` | 176.000 ms | 109.916 ms | 37.55% |

These are per-query wall times. The prototype does not include Files model construction, content/filter resolution, rendering, or terminal scheduling. They do not establish a 45.63% reduction in application CPU. The fixture has few changed files; the percentage is not a promise for every repository or change set.

The existing locked Git tests also passed:

```bash
LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo test --locked -p deltoids --features blob-resolve git::tests
```

Result: **29 passed**. Broad workspace gates remain part of implementation verification.

## Findings and concrete fixes

### 1. Correct the statement about rename configuration

The plan says status-side similarity can follow `diff.renames`. The tested `diff.renames=false` case still produces `Renamed`, as does the prototype using `DiffFindOptions::for_untracked(true)`.

Evidence: `git.rs:215` enables status renames on both columns. Pinned libgit2 `status.c:310` initializes the find flags with `GIT_DIFF_FIND_FOR_UNTRACKED`. Its `diff.h:717` places that bit inside `GIT_DIFF_FIND_ALL`; `diff_tform.c:264` consults configuration only when those find bits are absent. The runtime fixture confirms this behavior.

Fix: preserve the concrete current options: merged patch `renames(true)`, status-side `for_untracked(true)`. Keep the false/copies configuration regression cases. Do not substitute the default/config-driven finder. The corrected prototype passes both configuration cases.

### 2. Specify conditional index refresh

The plan leaves the index refresh operation unspecified. Forced loading increases the prototype's query time by approximately 14 ms in this fixture.

Evidence: pinned libgit2 `diff_generate.c:1406` uses `git_index_read(index, false)` for the current patch path. The retained-handle fixture proves that `read(false)` sees external staging and unstaging without changing net patch text. Both benchmark policies still pass the original 30% performance target.

Fix: use `index.read(false)` against the repository's cached index, passing that object explicitly to all diff constructors. Keep index writes disabled. The retained-handle test proves freshness and read-only behavior for ordinary external Git writes.

### 3. Reject damaged HEAD references

The old patch method treats every `head()` error as an empty tree (`git.rs:166`), while the existing status reader accepts only missing/unborn HEAD and rejects other failures (`status.c:289`). Copying the broad patch error handling into the aggregate reader would hide a status failure.

Evidence: the damaged-head fixture writes an invalid object ID into the current branch reference. Existing status returns a Reference-class error. The corrected prototype rejects that error instead of presenting every index entry as an addition.

Fix: accept only `ErrorCode::NotFound` and `ErrorCode::UnbornBranch` as an empty baseline; propagate other HEAD errors. Keep this change confined to the new aggregate reader. The fault probe demonstrates the corrected outcome.

### 4. Specify byte-path pairing and its cost

The plan identifies index-path pairing but leaves the algorithm open. A nested search through the two delta lists is quadratic in changed-file count, introducing avoidable work into a CPU fix.

Evidence: pinned libgit2 `diff_generate.c:1621` pairs staged `new_file.path` against workdir `old_file.path`, using case-insensitive comparison only when both diffs are sorted that way. The prototype sorts those byte keys and advances through them once; the 1,000-file fixture returns all matching records correctly.

Fix: use sorted pairing with byte paths and the corresponding ASCII case comparator; convert paths to the existing public string representation after pairing. The prototype's matching cost is O(n log n), not O(n²). Do not use Unicode lowercasing for Git path comparison.

### 5. Describe the actual dependency/runtime and portable test limit

`Cargo.lock:1146` pins `libgit2-sys 0.18.5+1.9.4`; this machine runs libgit2 1.9.7. The build script can select a compatible system library (`build.rs:113`). Lockfile version alone does not identify the executing C library.

The local filesystem rejects the invalid-UTF-8 filename fixture with `EILSEQ`. That case is skipped explicitly; it is not a pass.

Fix: record `git2::Version::get().libgit2_version()` in performance evidence and keep baseline/candidate on the same runtime. Run the byte-path fixture on a Linux filesystem that permits those names during implementation. The inspected 1.9.4 source supports the chosen public operations, but a vendored 1.9.4 runtime was not executed during this review.

## Strategy and remaining gates

The repository scan is the measured hot path in the earlier investigation. This prototype crosses the proposed owned-data interface without exposing Git objects or private status-list internals. Git and filesystem dependencies are exercised with temporary real repositories; no speculative adapter was added.

Lazygit's alternative is status for the whole repository plus detailed diffs restricted to selected paths. Its current source is under `/Users/juan/workspace/jesseduffield/lazygit` at `ff375b124`. That design remains a possible later optimization. Deltoids' unified scrolling view currently builds all file bodies, so adopting it would expand the selected proposal's scope.

Current compatibility is not proof that all current UI labels are ideal. For example, a chained rename's existing status key is the intermediate name, and conflict-only status records are omitted by the current public mapping. The prototype preserves those results. Correcting those behaviors is separate from proving this performance change.

The review required implementation checks for bounded snapshot retries during concurrent writes, failure retention in the real Files reload path, startup behavior, staging-only redraw, whole-TUI CPU/latency, focus/idle behavior, and the normal workspace gates. Completed results are recorded in `background-cpu.md`. Preserve tests for the unchanged public patch/status interfaces; replace only private tests made redundant by the new snapshot tests.

## Reproduction

Release artifact names change between builds. The exact successful compile using an available git2 0.21 artifact was:

```bash
LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib rustc --edition=2024 -O docs/investigations/shared-scan-plan-probe.rs --extern deltoids=target/release/deps/libdeltoids-65171fa05f41b693.rlib --extern git2=target/release/deps/libgit2-da6b9f36046c9870.rlib -L dependency=target/release/deps -L native=/opt/homebrew/opt/libgit2/lib -o /tmp/deltoids-shared-scan-plan-probe
python3 docs/investigations/shared-scan-plan-fixtures.py
/tmp/deltoids-shared-scan-plan-probe <temporary-large-repository> 20
/tmp/deltoids-shared-scan-plan-probe <temporary-large-repository> 20 force
```

The fixture driver prints its temporary root and saves `results.json` there. The optional `0 refresh` probe mode stages and unstages `a.txt`; use it only on a disposable fixture. The optional `0 head-error` mode expects a damaged HEAD fixture.

Public documentation checked:

- [libgit2 merge contract](https://libgit2.org/docs/reference/v1.9.0/diff/git_diff_merge.html).
- [git2 0.21 Diff interface and mutation rules](https://docs.rs/git2/0.21.0/git2/struct.Diff.html).
- [libgit2 rename-finder flags](https://libgit2.org/docs/reference/main/diff/git_diff_find_t.html).

Native source references above are under `/Users/juan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libgit2-sys-0.18.5+1.9.4/libgit2/`. Runtime behavior, rather than documentation alone, establishes the corrections.
