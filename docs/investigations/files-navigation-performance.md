# Files navigation performance

Files navigation now shows highlighted content in about 11 ms at p95 on the fixed Rust and TypeScript fixtures. The baseline took 106–120 ms. Revisits take less than 1 ms at p95. Both navigation probes and both stress probes pass their assertions.

Measurements use an Apple M1 Pro, macOS, Rust 1.92.0, release builds, TokyoNight in dark mode, and a 120 × 40 terminal. The baseline is commit `4a240a6`; the current build includes the changes accompanying this report. Runs were sequential, with compilation finished before timing.

## Navigation results

All values below are milliseconds. Each traversal contains 109 selections across 110 files, each with 300 generated Rust or TypeScript functions. First traversal includes directional preparation. Reverse traversal includes retained previews and complete blocks.

| Language | Observable | Baseline p50 | p95 | p99 | Current p50 | p95 | p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust | Selected file header | 0.37 | 0.48 | 2.37 | 0.47 | 0.63 | 0.66 |
| Rust | First traversal content | 103.29 | 105.81 | 110.66 | 9.08 | 11.02 | 11.12 |
| Rust | Reverse traversal content | 0.64 | 0.74 | 0.83 | 0.54 | 0.62 | 0.69 |
| TypeScript | Selected file header | 0.39 | 0.45 | 0.47 | 0.43 | 0.57 | 0.80 |
| TypeScript | First traversal content | 118.43 | 119.73 | 119.98 | 1.10 | 11.07 | 11.18 |
| TypeScript | Reverse traversal content | 0.68 | 0.78 | 0.81 | 0.51 | 0.63 | 0.72 |

A fresh process also jumps from the first file to the last file, beyond the preparation radius:

| Language | Baseline content | Current content |
| --- | ---: | ---: |
| Rust | 101.45 | 9.15 |
| TypeScript | 115.61 | 11.12 |

The header was already fast. The improvement is in content availability: Files no longer waits for the input settle timeout or highlights the entire file on the UI thread.

## Large files and directories

The stress fixture contains a 10,000-function file, two ordinary root files, and a directory with 100 ordinary files. It performs 100 rapid transitions through the large file, mouse selection, directory scrolling, width changes, focus loss and return, a real foreground command, and quitting with rendering pending. Large-file content timing uses a fresh process after the rapid transitions.

| Observable | Rust | TypeScript |
| --- | ---: | ---: |
| Large selection header p99 | 5.02 ms | 0.65 ms |
| Navigation away from large selection p99 | 10.25 ms | 10.69 ms |
| Cold large-file first usable content | 48.45 ms | 47.89 ms |
| Large-file complete body | 648.25 ms | 1132.54 ms |
| Mouse selection | 0.68 ms | 0.70 ms |
| Directory first usable content | 11.28 ms | 10.75 ms |
| Last directory viewport after repeated end-scroll input | 195.77 ms | 343.76 ms |
| Navigation out of the directory | 11.77 ms | 3.23 ms |
| Quit with pending rendering | 9.11 ms | 8.49 ms |

Foreground commands ran and the Files view repainted after terminal restoration in both runs. Settled idle and focus-lost intervals emitted zero terminal bytes. Process CPU time increased by 0.00 seconds during each 0.3-second quiet sample; `ps` reports hundredths of a second on this machine.

The directory end-scroll measurement includes repeated input as the assembled window grows. It is a single content-readiness measurement. The header and navigation-away percentiles measure input response across the rapid transitions. Quit timing is one sample per run.

A separate release probe assembles 30,699 rows from 100 files. Initial pending-window work took 6.33 ms. Completing all render jobs took 1.21 seconds. Assembly from complete blocks took 10.04 ms; reusing that window took 10.54 µs. These measure window computation, with no terminal output.

## What changed

- One private Files render module owns a worker, bounded scheduling, retained blocks, cancellation, and result validation. Computed diffs are shared through `Arc`; jobs copy small file metadata.
- Draws submit demand and read ready rows. Pending work shortens input polling; visible completions request frames independently of input settling.
- An uncached selection interrupts preparation. Nearby small files are prepared in travel order. Directory scheduling starts with the file at the viewport's top.
- Large hunks publish a usable prefix before returning their complete rows. Preview rows retain the final highlighting and source identities. The footer shows “Rendering…” while visible blocks remain incomplete.
- Cancellation checks run between source lines and intraline pair comparisons. Generation invalidation rejects obsolete results after reloads, render-setting changes, and pauses. Shutdown does not join the renderer.
- Selected windows are retained across cursor and status draws. Comment revisions invalidate overlays. Cursor targets remain intact while their rows are pending.
- Terminal frames use buffered writes, including the backend recreated after foreground commands.

## Memory and limits

The retained-block budget and byte accounting are defined in [render.rs](../../crates/deltoids-cli/src/cli/browse/files/render.rs). The budget counts estimated owned row storage. Visible blocks are pinned; speculative blocks are evicted first. The budget excludes the assembled window, model, syntax tables, queued results, and allocator overhead. A selected directory can exceed it. Retention-pressure tests verify that selection remains usable and preparation stops after completion.

Actual TUI resident memory, in MiB:

| Language | Baseline after all visits | Current after all visits | Baseline cold jump | Current cold jump |
| --- | ---: | ---: | ---: | ---: |
| Rust | 71.39 | 72.88 | 35.06 | 35.69 |
| TypeScript | 101.41 | 75.27 | 40.38 | 40.00 |

Cancelled files can retain usable previews while their complete bodies remain unfinished. Reselecting them can start full rendering again. This affects the memory comparison and separates first usable content from complete readiness.

These probes time arrival of a header or highlighted marker at a pseudo-terminal reader. A separate process drains output independently of Python screen decoding. They do not measure physical screen presentation. Fixture creation, startup model computation, and Git scanning are outside navigation timing. Larger inputs and different machines can take longer to finish.

## Reproduce

Build and run the current checks from the repository root:

```sh
env LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo build --release -p deltoids-cli
uv run --with pyte docs/investigations/files-navigation-probe.py --assert-fast
uv run --with pyte docs/investigations/files-navigation-probe.py --language ts --assert-fast
uv run --with pyte docs/investigations/files-navigation-stress.py
uv run --with pyte docs/investigations/files-navigation-stress.py --language ts
```

The Homebrew library path supplies libgit2 on this machine. Use the local platform's linker setup elsewhere. The probes create and remove temporary repositories and print JSON measurements.

For the baseline, build the detached commit and pass its executable to the same navigation probe:

```sh
navigation_baseline=$(mktemp -d /tmp/deltoids-navigation-baseline.XXXXXX)
git worktree add --detach "$navigation_baseline" 4a240a6
env LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo build --release -p deltoids-cli --manifest-path "$navigation_baseline/Cargo.toml"
uv run --with pyte docs/investigations/files-navigation-probe.py --binary "$navigation_baseline/target/release/deltoids"
uv run --with pyte docs/investigations/files-navigation-probe.py --binary "$navigation_baseline/target/release/deltoids" --language ts
git worktree remove "$navigation_baseline"
```

Correctness and computation checks:

```sh
cargo fmt --all -- --check
env LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo test --workspace -- --test-threads=4
env LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo clippy --workspace --all-targets
env LIBRARY_PATH=/opt/homebrew/opt/libgit2/lib cargo test --release -p deltoids-cli --lib navigation_frame_costs -- --ignored --nocapture
```

All checks passed: 872 workspace tests, the canonical diff-case suite, and the ignored directory computation probe. Tests cover pending navigation, reused file indices after reload, render epochs, hidden rendering and resumption, retention pressure, progressive directory completion, cursor identity, comments, wrapping, both change layouts, and existing special-file views. Preview tests compare text, styles, and source identities with complete rendering and verify cancellation after publication.
