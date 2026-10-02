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

The browse suite passes 280 tests. Full workspace tests, production Clippy, formatting, and diff checks pass. Default all-target Clippy remains blocked by the unchanged test nesting lint described above; all-target Clippy passes with that lint allowed. Focus deferral and conditional drawing remain deferred.
