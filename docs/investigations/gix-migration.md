# git2 → gix migration

## Verdict

`deltoids::git::Repo` reads repositories through `gix` 0.90 instead of
`git2` 0.21 (libgit2). Its public interface is unchanged. Parity holds on 34
fixtures; the differences below are deliberate. The working-tree snapshot is
about 2.5× faster, commit patches about 3.8× faster, and in-place writes no
longer crash the scan. The release binary grows about 4.4 MB.

## Method

`gix-parity.py` builds 34 fixture repositories with the git CLI, runs a dump
binary on each, and compares two dumps after shortening `index` ids to 7
characters (libgit2 abbreviated working-tree ids, the new writer prints full
ids). `gix-dump.rs` is that binary: it prints every `Repo` result for a
repository as JSON.

```bash
# One binary crate per side, each depending on its checkout's crate:
#   deltoids = { path = "<checkout>/crates/deltoids", default-features = false, features = ["blob-resolve"] }
# with gix-dump.rs as src/main.rs.
python3 docs/investigations/gix-parity.py make /tmp/fx
python3 docs/investigations/gix-parity.py run /tmp/fx <base-gitdump> base.json
python3 docs/investigations/gix-parity.py run /tmp/fx <new-gitdump> new.json
python3 docs/investigations/gix-parity.py diff base.json new.json
<gitdump> <repo> bench 20          # ms per working_tree_snapshot
<gitdump> <repo> commit-bench 20   # ms per commit_patch of HEAD
```

Baseline: `444f6a34` (git2). Measured on macOS, Apple Silicon, release
builds, 20 warm iterations.

## Parity

23 of 34 cases are equal. The other 11:

| Case | git2 | gix | Why |
| --- | --- | --- | --- |
| unborn repo, damaged `HEAD`: `commits` | error | error, other text | error messages differ |
| damaged `HEAD`: `working_tree_diff` | every file as an addition | error | matches `working_tree_snapshot`; production reads only the snapshot |
| staged delete, file recreated | patch shows only the deletion | one modification | libgit2 dropped the untracked side when merging diffs |
| unstaged rename, edited or not | patch shows delete + add while status says renamed | rename in both | untracked files are rename targets in the patch too |
| chained rename (staged, then moved again) | delete + add | rename to the final path | same |
| file → symlink, unstaged | new id `0000…`, `+++ /dev/null` | symlink's blob id, `+++ b/<path>` | same output as a staged typechange |
| merge conflict | `old mode 0` / `new mode 100644` | the file with its conflict markers against `HEAD` | libgit2 printed index stages |
| `core.autocrlf=true` edit | hunks of the converted text, id of the converted text | hunks and id of the raw CRLF bytes | the id matches the file on disk, so content resolves |
| commit order with equal timestamps | id order | `git log` order | gix walks like git |
| `history` commit patches | | | only listed in the other order; all 8 patches and the tree patch are byte-equal |

## Measurements

| | git2 | gix |
| --- | ---: | ---: |
| `working_tree_snapshot`, 30,000 files, 6 changed | 52–57 ms | 21–22 ms |
| `commit_patch`, 5 of 30,000 files changed | 1.65–1.67 ms | 0.43–0.46 ms |
| clean release build of `deltoids-cli` | 14.8 s | 18.7 s |
| `deltoids` binary | 32.2 MB + 1.0 MB libgit2 dylib | 37.6 MB |
| packages in `Cargo.lock` | 359 | 430 |

The git2 build linked Homebrew's libgit2 1.9.7 dynamically; release builds
vendor it statically, so the like-for-like size difference is about 4.4 MB.

## Crash and filters

- The `deltoids` lib tests include a writer that truncates and rewrites a
  file in place during snapshots. With git2 the lib test binary died of
  SIGBUS in about 1 of 16 runs (PR #27). With gix, 500 consecutive runs
  passed.
- `blob_filtered` runs filter drivers in process. A real git-crypt blob
  (stored form starting with `\0GITCRYPT\0`) decrypted to the exact
  working-tree plaintext.
