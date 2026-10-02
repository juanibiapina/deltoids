# Background CPU plan verification

## Verdict

The optimization strategy is supported by the investigation. The original plan needed six corrections before implementation: preserve watcher recovery signals and bound notification delivery, exclude hidden deadlines from polling, use stable trace cache identities, cover both terminal lifecycle implementations, preserve trace parser compatibility, and make recovery timing explicit.

The saved plan has been updated with these corrections. The review itself changed no application implementation. CPU targets remained acceptance goals at review time. The first scoped implementation and its measurements are recorded in `background-cpu.md`.

## Subsequent decision: trust healthy watchers

The user challenged the proposed 30-second reconciliation timer. That interval was a precaution and had no reproduced silent-event-loss failure supporting it. The plan now requires zero periodic full scans for healthy local filesystem watchers. The findings below describe the reviewed version; references to a 30-second timer are superseded by this decision.

Pinned notify 8.2 emits `Flag::Rescan` for macOS dropped-event hints (`fsevent.rs:117`) and Linux `Q_OVERFLOW` (`inotify.rs:212`). The current application discards this flag. Preserving that signal, detecting accumulator overflow and watcher errors, and installing watchers before initial snapshots provides concrete recovery paths. The current shell builds before arming at `browse/mod.rs:508`, then line 511, so correcting that order also closes a demonstrated startup notification gap.

The revised plan reconciles after explicit rescan/overflow/ambiguous-directory signals or successful watcher recovery. Installation failure is visible and retried with bounded backoff. Clean focus return only redraws; pending changes drive data refresh. There is no timer-driven full-scan fallback.

Baseline evidence and reproduction measurements live in [background-cpu.md](background-cpu.md).

## Findings and corrections

### 1. High: watcher recovery signals disappear before the scheduler sees them

**Evidence:** `crates/deltoids-cli/src/cli/browse/watch.rs:37` sends only `event.paths` through an unbounded channel and discards callback errors. Traces repeats this pattern at `traces/mod.rs:776`. The pinned notify 8.2 FSEvents implementation emits `EventKind::Other` with `Flag::Rescan` after dropped events. The [notify Event documentation](https://docs.rs/notify/8.2.0/notify/struct.Event.html#method.need_rescan) requires treating every file as potentially changed after this flag.

The runtime probe passed a real pathless `Flag::Rescan` event through the current `path_warrants_reload` function. Result: `need_rescan=true`, `paths=[]`, `reload=false`.

The original plan described recovering overflow and bounding the ids retained while hidden, but left the unbounded callback channel in place. That channel can accumulate while a synchronous diff or foreground command blocks the consumer.

**Correction:** preserve event kind, rescan signals, and errors; coalesce before queueing. Use a bounded pending accumulator plus a nonblocking wake notification. On capacity overflow, retain one reconciliation flag. The proposed pending-id limit is 4,096; it is a design limit, not a measured optimum. Treat ambiguous directory/root events as reconciliation requests.

**Fix proof:** a prototype using `sync_channel(1)`, nonblocking `try_send`, and retained pending state processed 100,000 callbacks with one queued wakeup and the rescan request preserved. This proves the primitive combination; integration and saturation tests remain implementation gates.

### 2. High: retaining hidden dirty state can create a zero-timeout loop

**Evidence:** `browse/mod.rs:438` returns `DEBOUNCE_DELAY.saturating_sub(since.elapsed())` whenever the active mode is dirty. `reload_active_if_due` at line 518 clears that state by servicing a reload. Deferring the reload while keeping the expired timestamp would keep returning zero.

The runtime probe used the exact timeout expression with a dirty timestamp one second old. Result: `Duration::ZERO`.

`apply_events` at `browse/mod.rs:705` also reloads after each event in a burst; a later focus-loss event would arrive after an earlier expensive refresh had already run.

**Correction:** deferred hidden deadlines are ineligible to control the input wait. Use a blocking wait or a positive housekeeping timeout while hidden. Process the burst before deciding whether an expensive refresh is eligible. Add a test that advances time past the debounce while focus is lost and verifies positive blocking behavior and no reload.

**Fix proof:** removing the deferred deadline from the eligible-deadline set eliminates the branch demonstrated by the exact-expression probe. The future scheduler still needs a regression test through its interface.

### 3. High: preserving trace caches requires replacing position keys

**Evidence:** `traces/detail.rs:40` stores rows under `(usize, usize)`; line 142 uses `(state.trace_index, state.entry_index())`. `TraceStore::list_for_cwd` sorts by last timestamp, and `traces/reload.rs` replaces the sorted vector on refresh. The original plan called for selective invalidation but did not specify changing the keys.

A probe using the real `TraceStore` added entries to two traces, then appended to the older trace with `TraceStore::append`. The trace at position zero changed from `b` to `a`. A retained position-zero cache would therefore refer to a different history after the sort.

**Correction:** key cached entries by trace id, history generation, and cwd-filtered entry index. Retain width/layout/theme in the existing render epoch. Append-only updates retain the generation; replacement/truncation/rewrite advances it. Keep selection restoration by trace id and remove deleted histories' cache entries.

**Fix proof:** the real store demonstrated list position instability while the trace ids remained stable. The generation distinguishes histories replaced under the same id. Implementation tests must cover reorder, removal, replacement, and append preservation.

### 4. High: focus lifecycle and drawing changes must cover custom-command restoration

**Evidence:** focus reporting belongs to both `terminal.rs:14`/its Drop implementation and `browse/suspend.rs:70`/line 86. `run_foreground` at `suspend.rs:48` temporarily leaves the TUI, invokes another application, and recreates `Terminal` at line 63. Today the unconditional event-loop draw repaints the recreated terminal. Drawing only after view changes removes that implicit repaint.

The pinned crossterm implementation and [EnableFocusChange documentation](https://docs.rs/crossterm/0.29.0/crossterm/event/struct.EnableFocusChange.html) provide the required commands. The runtime probe emitted exactly `ESC[?1004h` and `ESC[?1004l`.

An isolated tmux `next-3.9` server with `focus-events on` forwarded focus loss/gain to child programs when switching windows and when the attached pseudo-terminal received outer focus sequences. The two windows recorded `ESC[O ESC[I` and `ESC[I ESC[O ESC[I ESC[O`, respectively. This verifies tmux forwarding, not physical GUI-window focus.

**Correction:** disable focus reporting before a foreground child, re-enable it on return, reset focus knowledge, reconcile the active view, and explicitly request a full repaint. Add tests for normal return and command-spawn failure. Use the crossterm commands rather than writing Unix escape sequences in production code.

**Fix proof:** the actual crossterm commands emit the expected protocol, and the isolated tmux probe forwards it. Source inspection establishes the second lifecycle path and its dependency on an implicit redraw.

### 5. Medium: incremental trace reads must preserve historical parser behavior

**Evidence:** `trace_store.rs:431` parses `contents.lines()`, skipping blank lines. `HistoryEntry` at line 401 supports the historical `summary` alias and defaulted fields. `TraceStore::append` at line 70 holds a writer lock, but readers do not take it and can observe a partial JSON record.

A probe using the real `TraceStore::read` found that a partial JSON record errors and a complete final JSON record without a newline succeeds. A reader that waits exclusively for newline termination would regress the latter behavior.

**Correction:** reuse `HistoryEntry` parsing; preserve complete newline-free final records, blank lines, aliases, and defaults. Keep partial JSON/UTF-8 tails without advancing the committed offset, and retry without relying on another notification. A newline arriving after an accepted record must not duplicate it. Keep unrelated historical content out of the retained catalog; entry fields include potentially large content, hunks, and diff strings.

**Fix proof:** the real parser establishes the compatibility requirements. Completed-tail and appended-newline cases are explicit test gates for the incremental reader. Offset handling and memory consumption remain to be measured during implementation.

### 6. Medium: recovery deadlines and proof harnesses were underspecified

**Evidence:** Files has a one-second startup loading deadline at `files/mod.rs:99`. `resolve_reload` at line 313 returns false even when an unchanged reload promotes loading to ready, and when persistent failure creates an error view. Therefore an optimized renderer cannot use the current boolean alone as its draw signal.

Traces reload at `traces/mod.rs:798` propagates errors; it currently has no equivalent persistent last-view recovery behavior. The original plan's retry description did not specify the first attempt or separate startup deadline. Its trace reconciliation description had no cadence. Its strict “recover within 30 seconds” wording also omitted debounce and synchronous computation time.

`crates/tests/tests/tui_cli.rs:24` pipes stdout; these tests exercise scripted rendering. They cannot prove interactive focus handling. The existing browse suite was run successfully: 261 passed, zero failed, 128 filtered out.

**Correction:** represent visible transitions and transient failures in the Mode refresh result. Retry initially after 200 ms, back off to at most one attempt per second, and schedule the startup deadline separately. Reconcile trace metadata on the 30-second interactive deadline without rereading unchanged histories. Specify that recovery computation starts by the deadline, then measure completion latency. Add a Unix pseudo-terminal harness with controlling-terminal setup, output draining, bounded waits, and failure cleanup.

**Fix proof:** passing current browse tests establishes the baseline; the inspected transition branches and piped harness establish the missing coverage. The existing CPU probes already demonstrate a working controlling-terminal launch. CPU improvement and UI-recovery latency require the future executable.

## Other verified decisions

- **Veracity and facts:** the original live sample and both controlled CPU mechanisms support reducing scheduled work. Staging an edited file through real Git produced an identical HEAD-to-workdir patch but different staging status through `Repo::working_tree_status`; the plan's staging refresh requirement is necessary.
- **Strategy and alternatives:** increasing the full-scan interval alone helps idle Files but leaves cross-project trace reloads and confirmed hidden work. Focus handling alone leaves terminals that do not forward focus events. The combination in the plan addresses both cases. A worker thread does not eliminate refresh work and remains conditional on measured return latency.
- **Setup:** the local stale Homebrew link path is recoverable with `LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib`. That setting worked for the release build and browse tests. Current integration tests are under `crates/tests/`.
- **Consistency and module depth:** Files and Traces are existing Mode adapters. Scheduler policy is in-process state; the trace catalog and repository metadata use local filesystem/Git dependencies. The proposed modules hide demonstrated policy and I/O work. No speculative network port is needed.
- **Runtime capability:** a real linked worktree opened through pinned git2 0.21 had a separate `Repository::path`, the same `commondir` as the main repository, and a resolving `head`. These methods support the planned metadata lookup. Linux/macOS filesystem backends expose recovery signals through notify. Focus commands are available in pinned crossterm; Windows command handling is implemented by crossterm, while the Unix pseudo-terminal harness is platform-specific.
- **Future changes:** stable trace identities and bounded callback state survive sort changes and notification storms. Complete historical records retain their existing format. The current store puts content and hunks in JSONL, so the revised plan removes the unsupported separate blob-notification optimization.

## Reproduction artifacts and limits

[background-cpu-plan-probe.rs](background-cpu-plan-probe.rs) checks the real repository wrapper, real trace store, current path filter, notify rescan payload, callback coalescing primitive, timeout expression, and crossterm commands. It requires the workspace's built dependency archives. On the investigation machine:

```bash
rustc --edition=2024 docs/investigations/background-cpu-plan-probe.rs \
  -o /tmp/deltoids-verify-probe \
  -L dependency=target/release/deps \
  -L native=/opt/homebrew/opt/libgit2/lib \
  --extern deltoids=target/release/deps/libdeltoids-65171fa05f41b693.rlib \
  --extern deltoids_cli=target/release/deps/libdeltoids_cli-ca875bf7d99acbc0.rlib \
  --extern notify=target/release/deps/libnotify-27e1d43f8ade7009.rlib \
  --extern crossterm=target/release/deps/libcrossterm-d75e08e29c7108cb.rlib \
  --extern serde_json=target/release/deps/libserde_json-fd485cf5f4eb4b3f.rlib \
  --extern git2=target/release/deps/libgit2-c7a96d70a051e04d.rlib
probe_dir=$(mktemp -d /tmp/deltoids-plan-proof.XXXXXX)
/tmp/deltoids-verify-probe "$probe_dir"
```

Archive hashes are specific to this build. Use mutually compatible archives from one Cargo build; selecting each crate's newest archive independently selected incompatible serde dependency graphs during the review.

[background-tmux-focus-probe.py](background-tmux-focus-probe.py) creates a private socket/server, attaches a pseudo-terminal client, records focus sequences in two windows, and kills its server in `finally`:

```bash
python3 docs/investigations/background-tmux-focus-probe.py
LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo test -p deltoids-cli --lib cli::browse -- --quiet
```

The review proved runtime capabilities and specific current behaviors. It did not build the optimization, verify physical GUI terminal forwarding, measure future steady-state memory, or establish future CPU targets. Those are explicit implementation gates in the revised plan.
