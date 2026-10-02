# Background CPU investigation

## Baseline finding

Two refresh paths cause CPU usage without user input:

1. Files mode computes a full working-tree diff about once a second, even in a clean repository. A controlled repository with 30,000 tracked files used **9.12% CPU** while idle.
2. Traces mode reloads histories after changes anywhere in the shared trace directory. In a controlled store containing 50,000 unrelated history entries, traced edits in another project caused **12.37% CPU** in the watching process.

The TUI has no terminal focus handling. Both paths continue when the terminal reports focus loss. Redrawing every event-loop iteration adds work, but the measured expensive paths are Git scanning and trace loading.

## Scope and method

Investigation date: 2026-10-01. Checkout: `af04f7a1120e81d3306454a124045f6c6315fd53`, equal to `origin/main` after `git fetch`.

Measurements use macOS process CPU time divided by elapsed wall time; 100% means one full CPU core. Each controlled measurement lasts eight seconds after startup has settled. These are diagnostic measurements, not statistical benchmarks.

The reproduction scripts launch the real release executable in a pseudo-terminal with a controlling terminal and a 120 × 40 screen. They continuously drain terminal output, isolate configuration and trace storage using XDG paths, and create temporary Git repositories. They send `q` and stop their own processes on successful completion. Temporary fixtures remain for inspection. The trace probe measures CPU in the watching TUI, excluding the writer subprocesses.

No application source was changed during the baseline investigation. The files beside this report are investigation instruments. The implementation validation below records the first scoped fix.

## Existing processes

Initial process listing:

| PID | Working directory | Instantaneous CPU | Accumulated CPU | Elapsed lifetime |
| --- | --- | ---: | ---: | ---: |
| 62964 | `lovely-girl-house` | 27.2% | 7:01.12 | 1:12:16 |
| 48737 | `starmux` | 0.1% | 0:19.80 | 55:02 |
| 66763 | `deltoids` | 0.1% | 0:00.47 | 00:54 |

All three executable paths were `/Users/juan/.cargo/bin/deltoids`. `lsof` showed older executable inodes for the first two processes, so the same path does not establish that they run the same build.

A later snapshot showed PID 62964 at 9.4% CPU, with 7:30.20 accumulated over 1:17:51. The investigation did not establish the terminal focus state of these existing processes or attribute the initial 27.2% spike to a specific edit.

A five-second stack sample of PID 62964 captured:

```text
deltoids_cli::cli::browse::run
  FilesMode::reload
    files::reload::reload_working_tree
      deltoids::git::Repo::working_tree_diff
        git2::Repository::diff_tree_to_workdir_with_index
          git_diff_index_to_workdir
            filesystem_iterator_advance_into
              filesystem_iterator_frame_push
                lstat / git_vector_sort
```

Of 4,166 main-thread sample observations, 3,816 were in the blocking input poll and 337 in Files reload; 336 of those reload observations were in `working_tree_diff`. Sample counts include waiting time and must not be read as CPU percentages. The raw sample is `/tmp/deltoids-62964.sample.txt` on the investigation machine.

## Controlled Files reproduction

The release binary was rebuilt from the checkout above. A stale Cargo link path referenced Homebrew libgit2 1.9.4; adding the installed libgit2 directory via `LIBRARY_PATH` allowed the build to finish.

| Input | Watching TUI CPU |
| --- | ---: |
| Small clean repository, idle | 0.12% |
| 30,000 tracked files, clean and idle | 9.12% |
| Same process after sending focus loss (`ESC [ O`) | 8.12% |
| Same process, ignored file rewritten every 300 ms | 9.50% |
| Same process, tracked file rewritten every 300 ms | 28.73% |

An earlier run with the existing release artifact gave 9.62% idle, 9.12% after focus loss, and 28.50% during tracked edits. The rebuilt executable reproduces the same mechanism.

The idle condition needs no edits. Changing the repository size exposes the timer's cost. Ignored writes provide a control: their CPU stays around the large-repository idle baseline. Tracked writes exercise additional watcher-driven refreshes.

### Cause

`crates/deltoids-cli/src/cli/browse/mod.rs:95` sets `GIT_POLL_INTERVAL` to one second. At line 225 the loop marks Files dirty on each interval; `reload_active_if_due` at line 518 calls the mode's reload after the debounce. Files requests polling at `files/mod.rs:799`.

`files/reload.rs:124` calls `repo.working_tree_diff()` before comparing the patch against the previous input. An unchanged patch avoids rebuilding the model, but still incurs a complete filesystem scan. `crates/deltoids/src/git.rs:152` enters the libgit2 diff path observed in the live stack sample.

`terminal.rs:16` enters the terminal session without enabling focus reporting. `browse/mod.rs:716` handles keys and mouse events and ignores focus events. The injected focus-loss measurement exercises this omission; it does not test how a physical terminal or tmux forwards focus events. The local tmux server reports `focus-events on`.

The loop calls `terminal.draw` unconditionally at `browse/mod.rs:155`. Ratatui can suppress unchanged cells, but still builds the frame. Idle probe output contains approximately 725–825 bytes per eight-second interval, even when the displayed content is stable.

## Controlled Traces reproduction

The trace probe creates one local trace and an unrelated project's trace using the real `deltoids write` command. It populates 500 additional unrelated traces with 100 entries each, using a serialized entry from that command. It opens Traces and changes only the source of subsequent notifications.

| Input | Watching TUI CPU |
| --- | ---: |
| Traces idle with 50,000 unrelated historical entries | 0.12% |
| Ordinary writes in the unrelated project every 300 ms | 0.12% |
| `deltoids write` in the unrelated project every 300 ms | 12.37% |

This proves cross-project trace activity can cause substantial CPU usage. It does not measure the size or composition of the user's real trace store.

### Cause

`traces/mod.rs:772` installs a recursive watcher on the shared trace root. `should_reload` at line 787 accepts every notification. `reload_traces` invokes `load_traces_for_cwd`, which calls `list_traces_for_current_directory`.

`trace_store.rs:125` implements `list_for_cwd` by loading all histories before filtering entries by cwd. `load_all_raw` at line 179 reads and parses every valid trace's `entries.jsonl`. `traces/model.rs:15` then reads histories belonging to the selected project again. `traces/reload.rs` clears the diff cache on every reload, including reloads caused by unrelated projects.

The ordinary-write control rules out another project's filesystem writes alone as the cause in this setup. Recording those writes in the shared trace store triggers the expensive path.

## Reproduce

From the repository root on macOS:

```bash
LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo build -p deltoids-cli --release
python3 docs/investigations/background-cpu-probe.py
python3 docs/investigations/background-trace-cpu-probe.py
```

The scripts default to `target/release/deltoids`. Set `DELTOIDS_CPU_BINARY` to test another executable. They print fixture roots and CPU measurements as JSON. Run them sequentially to reduce measurement interference. Each creates a fresh fixture; the Files probe writes 30,000 small files, and the Traces probe creates 50,000 historical entries.

## Outcome

The idle Git scan and unrelated-project trace reload are confirmed causes. The live sample connects the user's busy Files process to the same Git scan path. The controlled experiments reproduce approximately 9% idle CPU and approximately 12% CPU from unrelated traced edits. Optimization should reduce scheduled work and defer expensive refreshes while unfocused.

## First fix: event-driven refresh

The implementation removes the one-second full Git scan. Files watches working-tree edits and relevant Git metadata, including shared refs outside linked worktrees. Callback batches retain at most 4,096 distinct paths; overflow and backend rescan signals request a reconciliation. Backend errors, dropped callbacks, and removed watch roots trigger visible watcher recovery with bounded backoff. Transient model reads retry explicitly. Staging status refreshes when the patch is unchanged.

Both modes install their watcher before reading the initial snapshot, retaining events that arrive during the read. Linux read-access events are ignored so diff reads cannot trigger another reload. No periodic full-scan fallback was added.

The existing 30,000-file fixture was reused after a fresh fixture-creation run timed out. The optimized release binary was launched with the same controlling pseudo-terminal, screen dimensions, and isolated configuration used for the baseline. A 35-second settled idle measurement produced:

| Case | Wall time | Process CPU time | Average CPU |
| --- | ---: | ---: | ---: |
| Clean repository with 30,000 tracked files | 35.00 s | 0.08 s | 0.23% |

The live probe then changed a tracked file and observed its new content without sending input, staged it and observed a terminal update, then committed it and observed “No local changes.” Full workspace tests passed. The final targeted browse run passed 274 tests, including real Git metadata notifications, linked worktree shared refs, removed watch roots, bounded batches, rescan delivery, and failure retries.

Formatting and production-code Clippy passed. Unmodified `traces/entries_pane.rs:194` has an existing `clippy::excessive_nesting` test lint that blocks the default all-target check. The all-target check passed with that single lint allowed.

At this checkpoint, focus deferral, selective trace-history reads, and redraw scheduling remained deferred. The release executable was built into `target/release/deltoids`.

## Second fix: selective trace loading

The Traces reader classifies existing histories once and retains parsed entry content only for the current project. Notifications accumulate at most 4,096 changed trace IDs. Lock-file notifications are ignored. Explicit rescans, ambiguous root notifications, overflow, and watcher recovery reconcile file metadata and read only new or changed histories.

A changed history is read in full to validate its committed prefix, which detects rewrites that preserve inode or length and rewrites that grow the file. An unchanged prefix reuses earlier parsed records and parses only appended records. Unrelated histories retain compact metadata, a committed offset, and a prefix fingerprint. Large changed histories still incur a full byte read; this implementation removes global rereads and repeated parsing of their earlier records.

Complete records without a final newline remain accepted. Blank lines, the historical `summary` alias, and defaulted fields use the shared parser. Incomplete JSON or UTF-8 retains its uncommitted offset and retries without another event. Malformed complete records preserve the last successful snapshot and report a refresh error through the shell's existing retry policy.

Rendered rows use `(trace_id, project_entry_index)` as their identity. Refreshes retain the unchanged entry prefix for each trace and evict rewritten or removed entries. Appends preserve earlier renders, selection, and scroll even when activity reorders the list. A new newest trace still becomes selected.

The existing Traces CPU probe was run sequentially against the release executable from the first fix and the new release executable. Each run creates 500 unrelated histories with 100 entries each, uses an isolated configuration and a controlling pseudo-terminal, then records another project's traced writes approximately every 300 ms.

| Case | Before average CPU | After average CPU | Measurement interval |
| --- | ---: | ---: | ---: |
| Traces idle with 50,000 unrelated entries | 0.12% | 0.12% | 8 s each |
| Ordinary writes in another project | 0.12% | 0.12% | 8 s each |
| Traced writes in another project | 13.47% | 0.12% | 8.02 s before; 8.00 s after |

The traced-write measurement drops from 1.08 CPU seconds to 0.01 CPU seconds, a 99.1% reduction in average CPU. These are short macOS process measurements with 0.01-second CPU-time resolution; they establish the fixture's improvement, not a guarantee for every repository.

Baseline fixture: `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-qdyo74sn`. Updated fixture: `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-_oxhw8bg`.

`python3 docs/investigations/background-trace-refresh-probe.py` automatically verifies initial loading, selection of a new local trace, an appended local entry, and recovery after deleting the selected trace in the real release TUI. It waits for the first interactive frame before switching modes and asserts distinctive emitted text because terminal redraws emit only changed portions of lines. Filesystem changes receive no additional input.

The browse suite passes 280 tests. Full workspace tests, production Clippy, formatting, and diff checks pass. Default all-target Clippy remains blocked by the unchanged test nesting lint described above; all-target Clippy passes with that lint allowed. Focus deferral and conditional drawing remained deferred at this checkpoint.

## Third fix: focus deferral and conditional drawing

The terminal requests focus reporting on entry, disables it for foreground children and exit, and restores it when a child returns. Unknown focus permits normal operation. Focus loss postpones refreshes, watcher recovery, history loading, and drawing. Notifications remain in bounded watcher accumulators until focus returns. Keyboard or mouse input resumes interaction if a focus-gain report is missing.

The shell processes an input burst before scheduling expensive work and retains late focus reports even after a custom command consumes its key. Background waits exclude expired dirty and retry deadlines. Focus return services pending active-mode work and repaints in full; inactive modes remain lazy. A clean focus return does not read histories or compute a new Git diff. Foreground-command return resets focus knowledge, services pending changes, and repaints the recreated terminal.

Startup, input, resize, popup/theme/layout changes, visible refreshes, and refresh-error changes request frames. A fast navigation frame gets one settled full frame. Other idle timeouts produce no drawings. Loading-to-clean and loading-to-error transitions now report a visible change even without a rebuilt patch.

The automated focus probe was run against the prior selective-trace release and the updated release in the existing 30,000-file fixture. The probe injects terminal focus sequences, writes a tracked file approximately every 300 ms, records local traces, and checks emitted terminal text and focus-control sequences.

| Case | Before average CPU | After average CPU | Before terminal bytes | After terminal bytes |
| --- | ---: | ---: | ---: | ---: |
| Focused Files idle | 0.12% | 0.00% | 800 | 0 |
| Focused tracked writes | 28.49% | 28.75% | 1,181 | 952 |
| Unfocused tracked writes | 29.98% | 0.00% | 1,266 | 0 |
| Unfocused local traced writes | 0.25% | 0.12% | 2,843 | 0 |

Each case lasts approximately eight seconds. The updated idle and unfocused Files cases record 0.00 process CPU seconds at `ps`'s 0.01-second resolution; this indicates work below that measurement resolution. Unfocused trace writes record 0.01 CPU seconds. The traced-write CPU difference is one measurement quantum, so the useful direct evidence there is deferred reads in the shell tests and zero terminal output during the real stream.

The latest file content appears 210.3 ms after Files regains focus. A new matching trace appears 0.7 ms after Traces regains focus. The probe asserts that a foreground child runs with focus reporting disabled, that the updated file appears after terminal restoration, that reporting is re-enabled, and that exit disables it again. It accounts for terminal updates that emit only changed characters and for histories whose timestamps have one-second resolution.

Baseline fixture configuration: `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-nqbo53kh`. Updated fixture configuration: `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-zpl2etw3`.

Run `python3 docs/investigations/background-focus-cpu-probe.py` for automated focus, idle, catch-up, child, and exit assertions. It creates a small temporary Git fixture by default. Set `DELTOIDS_FOCUS_REPO` to an existing temporary large fixture to repeat the large-repository measurements; the probe writes `main.txt` and `trace.txt` there. For an older binary, set `DELTOIDS_CPU_BINARY` and `DELTOIDS_FOCUS_EXPECT_DEFER=0` to collect baseline measurements without asserting the new behavior.

`python3 docs/investigations/background-idle-draw-probe.py` measures sixty settled seconds and asserts zero terminal bytes. It accepts the same temporary-fixture override. The updated release passed in the large fixture: 60.00 seconds, 0.00 process CPU seconds at the 0.01-second measurement resolution, and zero terminal bytes.

Full workspace tests and production Clippy pass. The browse suite now has 290 tests, covering clean returns, bounded retained work, late focus loss, deferred deadlines, input fallback, stable refreshes, loading transitions, and child-return repainting. Default all-target Clippy retains the existing test nesting limitation described above. At this checkpoint, physical GUI focus forwarding remained unverified; the injected-event proof and the earlier isolated tmux forwarding proof established the tested focus paths.

## Native focus verification and remaining active-edit cost

Native focus forwarding and actual background deferral are verified on the user's Ghostty client through tmux. Before the shared-scan change below, active-edit cost was dominated by two full repository queries: patch generation and sidebar staging/status collection.

### Actual application focus

The attached client reports `xterm-ghostty`, tmux reports `next-3.9`, and `focus-events` is enabled. The native recorder first switches between two tmux windows as a positive control. It then activates Finder and Ghostty through AppKit, verifies which application is frontmost, and observes real focus-loss (`ESC[O`) and focus-gain (`ESC[I`) notifications. The driver restores the original tmux session and foreground application afterward. No focus sequences are injected.

`python3 docs/investigations/background-native-focus-probe.py` repeats that check. It temporarily attaches the Ghostty client to an isolated recorder session, requires a running Finder and Ghostty, and records only focus sequences. The successful original log is under `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-native-focus-2llix94z`.

The separate native TUI probe runs the actual release executable on that attached client with isolated configuration. It checks the tmux screen and captures the executable's output while editing `main.txt` in the existing large fixture. A separate sampling phase captures the caller chain during focused edits.

| Real application state | Wall time | Process CPU time | Average CPU | Terminal bytes |
| --- | ---: | ---: | ---: | ---: |
| Ghostty foreground, tracked writes every 300 ms | 8.00 s | 2.37 s | 29.62% | 963 |
| Finder foreground, tracked writes every 300 ms | 8.00 s | 0.00 s | 0.00% | 0 |

The background CPU value is below `ps`'s 0.01-second resolution. While Finder remains foreground, the probe writes `NATIVE_RETURN_READY` and asserts that the TUI screen still lacks it. After native Ghostty activation, the TUI displays that latest content. Activation request to observed updated frame takes 426.4 ms, including the probe's 300 ms AppKit settling period and command-launch overhead; it is not directly comparable to the injected-event latency above.

Run `python3 docs/investigations/background-native-tui-probe.py` for this application-level check. Set `DELTOIDS_FOCUS_REPO` to a temporary large fixture for the large-repository case; the driver edits `main.txt`, temporarily switches the attached client and foreground application, then restores them. Default runs use a fresh small fixture.

### Named active-edit cause

The sampling artifact is `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-f544isfv/active-edit.sample.txt`, produced with `/usr/bin/sample` over a separate five-second focused-write phase. Of 1,229 main-thread samples within refresh branches, 620 enter `Repo::working_tree_diff` and 494 enter `build_model → stage_map → Repo::working_tree_status`. Together these account for 90.6% of refresh-branch samples. Sampling includes waiting and system calls, so these percentages describe sampled stacks rather than an exact division of CPU time.

The observed branches map to:

- `files/reload.rs:115` calls `Repo::working_tree_diff` for every refresh.
- `git.rs:177` runs `diff_tree_to_workdir_with_index`; its libgit2 iterator walks the working tree and calls `lstat`.
- For a changed patch, `files/model.rs:58` calls `stage_map`, which queries status again.
- `git.rs:224` calls `statuses`; the sample shows a second `git_diff_index_to_workdir` walk and more `lstat` calls.

The other 115 refresh samples enter content retrieval, mostly waiting for `git cat-file --filters` subprocesses. The sample records two additional main-thread samples in drawing. This fixture's first optimization target is repository refresh work.

A direct probe calls the real public repository methods twenty times each after warming them, on the same temporary repository:

| Method | Average wall time per call |
| --- | ---: |
| `Repo::working_tree_diff` | 89.20 ms |
| `Repo::working_tree_status` | 89.88 ms |
| Combined cost of the two queries | 179.08 ms |

The probe source is `docs/investigations/background-active-query-probe.rs`. It uses the release library artifact from this checkout. The tested compilation command is:

```bash
LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib rustc --edition=2024 -O docs/investigations/background-active-query-probe.rs --extern deltoids=target/release/deps/libdeltoids-afb92e041c260c61.rlib -L dependency=target/release/deps -L native=/opt/homebrew/opt/libgit2/lib -o /tmp/deltoids-active-query-probe
/tmp/deltoids-active-query-probe <temporary-repository>
```

The release artifact hash can change after rebuilding. These timings are average wall time in a warm, unchanged snapshot; they are separate from the real TUI CPU measurement.

### Implemented shared repository snapshot

Files startup and refresh now call `Repo::working_tree_snapshot` once. It supplies patch text and both staging columns from one index-to-workdir enumeration per successful attempt. A second HEAD-to-index diff against the same captured index preserves the independent status rename policy. Merge/printing use copied deltas; staging uses the retained raw workdir diff. The index is conditionally refreshed and never written by these reads. Files model construction receives the staging map and does not query status.

The public query probe above now measures the implemented snapshot as well. Twenty warm calls on the same approximately 30,000-file fixture, using native libgit2 1.9.7, yielded:

| Query | Average wall time |
| --- | ---: |
| Existing patch method | 95.09 ms |
| Existing status method | 89.22 ms |
| Combined separate queries | 184.31 ms |
| Implemented snapshot | 97.05 ms |

The implemented query is **47.35% faster** than the combined separate queries in this measurement. Compile the current probe with `--extern deltoids=target/release/deps/libdeltoids-65171fa05f41b693.rlib` after a release build; the artifact hash can change.

#### Real TUI comparison

A fresh baseline release was built with `cargo build --locked --release -p deltoids-cli` before editing production source, then copied to `/tmp/deltoids-shared-scan-baseline`. The candidate uses the same checkout dependencies, compiler, and system libgit2. Both ran sequentially on controlling PTYs against the same large fixture, with writes every 300 ms and no debounce or rendering-policy changes.

`shared-scan-tui-probe.py` checks actual output, counts emitted frame boundaries by the cursor-hide command issued for each draw, and asserts final content, quiet idle/background phases, and catch-up on focus return.

| Eight-second phase | Baseline | Candidate |
| --- | ---: | ---: |
| Focused edit CPU time | 2.27 s | 1.39 s |
| Focused edit average CPU | 28.30% | 17.34% |
| Produced writes | 26 | 26 |
| Emitted frames during edits | 12 | 13 |
| Focused idle CPU / output | 0.00 s / 0 bytes | 0.00 s / 0 bytes |
| Unfocused edit CPU / output | 0.00 s / 0 bytes | 0.00 s / 0 bytes |
| Final-content write to observed output | 622.5 ms | 485.4 ms |
| Focus return to latest content | 212.0 ms | 128.1 ms |

CPU while editing decreased by approximately **39%**, with one more delivered frame. Zero CPU readings are below the `ps` clock's 0.01-second resolution. These are fixture-specific eight-second measurements; latency values are single observations. They include Files model construction, content resolution, and drawing, which the direct query probe excludes.

Run the comparison with:

```bash
DELTOIDS_CPU_BINARY=/tmp/deltoids-shared-scan-baseline DELTOIDS_FOCUS_REPO=<temporary-large-repository> python3 docs/investigations/shared-scan-tui-probe.py
DELTOIDS_CPU_BINARY=<candidate-release-binary> DELTOIDS_FOCUS_REPO=<temporary-large-repository> python3 docs/investigations/shared-scan-tui-probe.py
```

The probe edits `main.txt`; use a disposable fixture. The final complete baseline run used evidence root `/var/folders/ks/t5mwll9d0ys7xs_ng16n_qkc0000gn/T/deltoids-cpu-s6w7f_a4`; the candidate run used `deltoids-cpu-yksv8l8w` under the same temporary parent. An earlier baseline attempt failed only because an incremental terminal update split its catch-up marker; the corrected marker shares no cells with the previous content. A subsequent attempt timed out and is excluded from the completed comparison above.

#### Correctness and gates

Repository-interface tests cover staging columns, net cancellation, external index writes through retained handles, unchanged index bytes/mtime, unborn/detached HEAD, additions/deletions/recreation, renames and configuration, case/chained renames, type changes, binaries, conflicts, filters, submodules, many changed files, and concurrent-write reads. Files Mode tests prove staging-only redraw, unchanged-snapshot suppression, view retention on a real damaged-reference failure, recovery after repair, and startup recovery from Loading.

The implemented library also passed the verification driver: 30 macOS fixture cases, with invalid-UTF-8 filenames unsupported locally. That gap is closed by **12 passing Linux snapshot tests**, including the actual invalid-UTF-8 filename test, in `rust:1.92-slim` with the pinned vendored libgit2 1.9.4. The container mounts source read-only and places Cargo output outside the workspace. Its log is `/tmp/deltoids-shared-linux/linux-tests.log`.

The Linux Cargo command is:

```bash
cargo test --locked --manifest-path /work/Cargo.toml -p deltoids --features blob-resolve,git2/vendored-libgit2 git::tests::snapshot
```

All workspace tests and the workspace build pass. Formatting, production Clippy, and `git diff --check` pass. Default all-target Clippy still reports the unchanged nesting violation in `traces/entries_pane.rs:194`; all-target Clippy passes with `-A clippy::excessive_nesting`.

The measured candidate was built at `target/release/deltoids`. After verification, the user installed the updated executable with `make`. Existing user sessions were not replaced. Selective per-file refresh and filter subprocess caching remain possible later work; this change shares work within each full refresh.
