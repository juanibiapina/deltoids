//! What the watcher watches on Linux, read from the process's inotify
//! watches in `/proc/self/fdinfo`, and what it reports.

use std::collections::HashSet;
use std::os::unix::fs::{MetadataExt, symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::*;

/// The inodes every inotify instance in this process watches. Tests run
/// in parallel, so ask only about directories of your own.
fn watched_inodes() -> HashSet<u64> {
    let mut inodes = HashSet::new();
    for entry in std::fs::read_dir("/proc/self/fd").unwrap().flatten() {
        let is_inotify = std::fs::read_link(entry.path())
            .is_ok_and(|target| target.as_os_str() == "anon_inode:inotify");
        if !is_inotify {
            continue;
        }
        let info = Path::new("/proc/self/fdinfo").join(entry.file_name());
        let Ok(info) = std::fs::read_to_string(info) else {
            continue;
        };
        for line in info.lines().filter(|line| line.starts_with("inotify wd:")) {
            if let Some(inode) = line
                .split_whitespace()
                .find_map(|field| field.strip_prefix("ino:"))
            {
                inodes.insert(u64::from_str_radix(inode, 16).unwrap());
            }
        }
    }
    inodes
}

fn is_watched(path: &Path) -> bool {
    let inode = std::fs::metadata(path).unwrap().ino();
    watched_inodes().contains(&inode)
}

/// Poll `done` for up to three seconds, failing with `what`.
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait until the watcher has applied every change made so far: it reads
/// the whole root each time, so once a new top-level directory is
/// watched, earlier changes are applied too.
fn settle(root: &Path) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let barrier = root.join(format!("barrier{}", NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&barrier).unwrap();
    eventually("the watcher took in the barrier", || is_watched(&barrier));
}

/// Write `path` until a batch holds it, failing on a watcher error, and
/// return every path seen on the way. Writing again covers a rescan,
/// which drops the paths it replaces.
fn wait_for_edit(receiver: &ChangeReceiver, path: &Path) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    eventually(&format!("{} was reported", path.display()), || {
        std::fs::write(path, "edit").unwrap();
        let batch = receiver.take();
        assert!(batch.error.is_none(), "{:?}", batch.error);
        seen.extend(batch.paths);
        seen.iter().any(|seen| seen == path)
    });
    seen
}

/// A repository with an ignored `target/x`, `.gitignore` committed.
fn repo_ignoring_target(root: &Path) -> TestRepo {
    let repo = init_repo(root);
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(root.join("src/a")).unwrap();
    std::fs::write(root.join("src/a/f"), "f").unwrap();
    stage_all(&repo);
    commit_index(&repo, "init");
    repo
}

fn watcher_at(root: &Path) -> ChangeWatcher {
    let repo = git::Repo::discover_at(root).unwrap();
    ChangeWatcher::start(&repo, None).unwrap().unwrap()
}

#[test]
fn startup_watches_unignored_top_level_trees_and_no_symlink_targets() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("repo");
    let outside = base.join("outside");
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    std::fs::create_dir_all(root.join("src/a/b")).unwrap();
    std::fs::create_dir_all(root.join("target/x/y")).unwrap();
    std::fs::create_dir_all(root.join(".direnv")).unwrap();
    std::fs::create_dir_all(root.join("vendor")).unwrap();
    symlink(&outside, root.join(".direnv/link")).unwrap();
    symlink(&outside, root.join("top_link")).unwrap();
    symlink(&outside, root.join("src/out_link")).unwrap();
    std::fs::write(root.join("src/a/b/f"), "f").unwrap();
    std::fs::write(root.join("vendor/lib.rs"), "lib").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n.direnv/\nvendor/\n").unwrap();
    let repo = init_repo(&root);
    repo.git(&["add", "-f", ".gitignore", "src", "vendor/lib.rs"]);
    commit_index(&repo, "init");

    let _watcher = watcher_at(&root);

    for watched in ["", "src", "src/a", "src/a/b", ".git", "vendor"] {
        assert!(is_watched(&root.join(watched)), "{watched:?} is watched");
    }
    for unwatched in ["target", "target/x", "target/x/y", ".direnv"] {
        assert!(!is_watched(&root.join(unwatched)), "{unwatched} is not");
    }
    assert!(!is_watched(&outside), "symlink targets are not watched");
    assert!(!is_watched(&outside.join("deep")));
}

#[test]
fn new_top_level_directory_is_watched_with_its_contents() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    repo_ignoring_target(&root);
    let watcher = watcher_at(&root);
    let receiver = watcher.receiver();

    std::fs::create_dir_all(root.join("newdir/sub")).unwrap();
    std::fs::write(root.join("newdir/sub/f"), "first").unwrap();
    eventually("the new directory is watched", || {
        is_watched(&root.join("newdir/sub"))
    });
    settle(&root);
    receiver.take();

    wait_for_edit(&receiver, &root.join("newdir/sub/f"));
    assert!(watcher.is_healthy());
}

#[test]
fn new_ignored_top_level_directory_stays_unwatched() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    repo_ignoring_target(&root);
    let watcher = watcher_at(&root);

    std::fs::create_dir_all(root.join("target/x")).unwrap();
    settle(&root);

    assert!(!is_watched(&root.join("target")));
    assert!(!is_watched(&root.join("target/x")));
    assert!(watcher.is_healthy());
}

#[test]
fn ignore_rule_and_index_changes_update_the_watches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = repo_ignoring_target(&root);
    std::fs::create_dir_all(root.join("target/x")).unwrap();
    let watcher = watcher_at(&root);
    assert!(!is_watched(&root.join("target")));

    std::fs::write(root.join(".gitignore"), "").unwrap();
    eventually("an unignored directory is watched", || {
        is_watched(&root.join("target/x"))
    });

    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    settle(&root);
    assert!(!is_watched(&root.join("target")), "ignored again");

    std::fs::write(root.join("target/x/keep"), "keep").unwrap();
    repo.git(&["add", "-f", "target/x/keep"]);
    eventually("a directory with a tracked file is watched", || {
        is_watched(&root.join("target/x"))
    });
    assert!(watcher.is_healthy());
}

#[test]
fn renamed_top_level_directory_reports_its_new_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    repo_ignoring_target(&root);
    let watcher = watcher_at(&root);
    let receiver = watcher.receiver();

    std::fs::rename(root.join("src"), root.join("lib")).unwrap();
    settle(&root);
    receiver.take();

    let seen = wait_for_edit(&receiver, &root.join("lib/a/f"));
    assert!(
        !seen.iter().any(|path| path.starts_with(root.join("src"))),
        "no event under the old name: {seen:?}"
    );
    assert!(watcher.is_healthy());
}

#[test]
fn removed_top_level_directory_keeps_the_watcher_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    repo_ignoring_target(&root);
    let watcher = watcher_at(&root);
    let receiver = watcher.receiver();

    std::fs::remove_dir_all(root.join("src")).unwrap();
    settle(&root);

    wait_for_edit(&receiver, &root.join("README"));
    assert!(watcher.is_healthy());
}

#[test]
fn edits_behind_symlinks_are_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("repo");
    let outside = base.join("outside");
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    repo_ignoring_target(&root);
    symlink(&outside, root.join("top_link")).unwrap();
    symlink(&outside, root.join("src/out_link")).unwrap();
    let watcher = watcher_at(&root);
    let receiver = watcher.receiver();

    std::fs::write(outside.join("file"), "edit").unwrap();
    std::fs::write(outside.join("deep/file"), "edit").unwrap();
    let seen = wait_for_edit(&receiver, &root.join("sentinel"));
    assert!(
        !seen.iter().any(|path| {
            path.starts_with(root.join("top_link")) || path.starts_with(root.join("src/out_link"))
        }),
        "events behind symlinks: {seen:?}"
    );
}
