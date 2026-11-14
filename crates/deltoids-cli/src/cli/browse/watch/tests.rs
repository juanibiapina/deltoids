//! Tests for the watcher, through [`ChangeWatcher::start`] and the
//! batch it feeds.

use super::*;

use crate::test_git::TestRepo;

#[cfg(target_os = "linux")]
mod linux;

fn init_repo(dir: &Path) -> TestRepo {
    TestRepo::init(dir)
}

fn stage_all(repo: &TestRepo) {
    repo.stage_all();
}

fn commit_index(repo: &TestRepo, msg: &str) {
    repo.commit(msg);
}

#[test]
fn path_warrants_reload_filters_ignored_and_git_internal() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
    stage_all(&repo);
    commit_index(&repo, "init");

    let wrapper = git::Repo::discover_at(dir.path()).unwrap();

    assert!(path_warrants_reload(
        &wrapper,
        &[dir.path().join("src/main.rs")]
    ));
    assert!(!path_warrants_reload(
        &wrapper,
        &[dir.path().join("node_modules/x.js")]
    ));
    assert!(!path_warrants_reload(
        &wrapper,
        &[dir.path().join(".git/index.lock")]
    ));
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::fs::write(dir.path().join("node_modules/kept.js"), "kept").unwrap();
    repo.git(&["add", "-f", "node_modules/kept.js"]);
    assert!(
        path_warrants_reload(&wrapper, &[dir.path().join("node_modules/kept.js")]),
        "a tracked file in an ignored directory reloads"
    );
    // A batch with at least one real path still reloads.
    assert!(path_warrants_reload(
        &wrapper,
        &[
            dir.path().join(".git/index.lock"),
            dir.path().join("src/main.rs"),
        ]
    ));
}

#[test]
fn is_git_internal_detects_dot_git() {
    assert!(is_git_internal(Path::new("/repo/.git/index")));
    assert!(is_git_internal(Path::new(".git/HEAD")));
    assert!(!is_git_internal(Path::new("/repo/src/main.rs")));
}
#[test]
fn repeated_events_are_coalesced_and_later_events_survive_take() {
    let (sender, receiver) = change_channel(None);
    for _ in 0..100_000 {
        sender.send(Ok(
            notify::Event::new(notify::EventKind::Any).add_path("same".into())
        ));
    }
    let batch = receiver.take();
    assert_eq!(
        batch.paths.into_iter().collect::<Vec<_>>(),
        vec![PathBuf::from("same")]
    );
    assert!(!batch.rescan);
    sender.send(Ok(
        notify::Event::new(notify::EventKind::Any).add_path("later".into())
    ));
    assert!(receiver.take().paths.contains(Path::new("later")));
}

#[test]
fn overflow_and_backend_rescan_retain_recovery_work() {
    let (sender, receiver) = change_channel(None);
    for index in 0..=MAX_PENDING_PATHS {
        sender.send(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(index.to_string().into())
        ));
    }
    let batch = receiver.take();
    assert!(batch.rescan);
    assert!(batch.paths.is_empty());
    sender.send(Ok(
        notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)
    ));
    assert!(receiver.take().rescan);
}

#[test]
fn callback_errors_and_disconnection_are_visible() {
    let (sender, receiver) = change_channel(None);
    sender.send(Err(notify::Error::generic("backend failed")));
    assert!(receiver.take().error.unwrap().contains("backend failed"));
    drop(sender);
    assert!(receiver.take().error.unwrap().contains("disconnected"));
}

#[test]
fn git_control_changes_reload_but_object_and_lock_churn_do_not() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let repo = git::Repo::discover_at(dir.path()).unwrap();
    for path in [
        "HEAD",
        "index",
        "packed-refs",
        "refs/heads/main",
        "config",
        "info/exclude",
    ] {
        assert!(
            path_warrants_reload(&repo, &[repo.git_dir().join(path)]),
            "{path}"
        );
    }
    for path in [
        "index.lock",
        "objects/ab/cd",
        "logs/HEAD",
        "refs/heads/main.lock",
        "gitbutler/gitbutler.write-lock",
        "gitbutler/but.sqlite",
    ] {
        assert!(
            !path_warrants_reload(&repo, &[repo.git_dir().join(path)]),
            "{path}"
        );
    }
}

#[test]
fn real_watcher_reports_edits_and_ignores_clean_time() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let watcher = start(&root, None);
    let receiver = watcher.receiver();
    assert!(receiver.take().paths.is_empty());
    let path = root.join("file.txt");
    std::fs::write(&path, "edit").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let batch = receiver.take();
        if batch.rescan || batch.paths.contains(&path) {
            break;
        }
        assert!(batch.error.is_none(), "{:?}", batch.error);
        assert!(
            std::time::Instant::now() < deadline,
            "edit was not observed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn reads_do_not_schedule_refreshes_but_write_closes_do() {
    let (sender, receiver) = change_channel(None);
    sender.send(Ok(notify::Event::new(notify::EventKind::Access(
        notify::event::AccessKind::Open(notify::event::AccessMode::Read),
    ))
    .add_path("file".into())));
    let batch = receiver.take();
    assert!(batch.paths.is_empty());
    assert!(!batch.rescan);
    sender.send(Ok(notify::Event::new(notify::EventKind::Access(
        notify::event::AccessKind::Close(notify::event::AccessMode::Write),
    ))
    .add_path("file".into())));
    assert!(receiver.take().paths.contains(Path::new("file")));
}
fn wait_for_relevant_change(repo: &git::Repo, receiver: &ChangeReceiver) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let batch = receiver.take();
        let paths: Vec<_> = batch.paths.into_iter().collect();
        if batch.rescan || path_warrants_reload(repo, &paths) {
            return;
        }
        assert!(batch.error.is_none(), "{:?}", batch.error);
        assert!(
            std::time::Instant::now() < deadline,
            "Git change was not observed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn actual_git_metadata_notifications_survive_root_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let git_repo = init_repo(dir.path());
    let repo = git::Repo::discover_at(dir.path()).unwrap();
    let watcher = ChangeWatcher::start(&repo, None).unwrap().unwrap();
    git_repo.git(&["config", "core.filemode", "false"]);
    wait_for_relevant_change(&repo, &watcher.receiver());
}

#[test]
fn linked_worktree_watches_shared_refs_outside_its_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    std::fs::create_dir(&main).unwrap();
    let git_repo = init_repo(&main);
    std::fs::write(main.join("file"), "initial").unwrap();
    stage_all(&git_repo);
    commit_index(&git_repo, "initial");
    let linked = dir.path().join("linked");
    git_repo.git(&["worktree", "add", "-q", linked.to_str().unwrap()]);
    let repo = git::Repo::discover_at(&linked).unwrap();
    let watcher = ChangeWatcher::start(&repo, None).unwrap().unwrap();
    let oid = git_repo.rev("HEAD");
    git_repo.git(&["update-ref", "refs/heads/another", &oid]);
    wait_for_relevant_change(&repo, &watcher.receiver());
}
/// A fake Rust language server for `root` that watches its `.rs`
/// files once initialized.
fn rust_server_watching(root: &Path) -> (crate::lsp::Lsp, crate::lsp::fake::Record) {
    let watchers = serde_json::json!([{ "globPattern": format!("{}/**/*.rs", root.display()) }]);
    crate::lsp::fake::lsp(
        root,
        serde_json::json!({}),
        crate::lsp::fake::watching(watchers),
    )
}

/// Poll `lsp` until `done`, for up to three seconds.
fn poll_until(lsp: &crate::lsp::Lsp, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        lsp.poll();
        if done() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn real_watcher_tells_the_language_servers_about_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let root = root.as_path();
    let (lsp, record) = rust_server_watching(root);
    let watcher = start(root, Some(lsp.file_events()));
    lsp.warm(deltoids::Language::Rust);
    poll_until(&lsp, "the server registered", || record.replied("register"));

    let path = root.join("lib.rs");
    std::fs::write(&path, "fn a() {}\n").unwrap();
    let uri = url::Url::from_file_path(&path).unwrap().to_string();
    poll_until(&lsp, "the server heard about the edit", || {
        record
            .params("workspace/didChangeWatchedFiles")
            .iter()
            .any(|params| params["changes"][0]["uri"] == uri.as_str())
    });
    assert!(
        watcher.receiver().take().paths.contains(&path),
        "the diff still reloads"
    );
}

#[test]
fn removed_watch_root_restarts_the_servers_that_watch_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir(&root).unwrap();
    let (lsp, record) = rust_server_watching(&root);
    let _watcher = start(&root, Some(lsp.file_events()));
    lsp.warm(deltoids::Language::Rust);
    poll_until(&lsp, "the server registered", || record.replied("register"));
    assert_eq!(record.launches(), 1);

    std::fs::remove_dir_all(&root).unwrap();
    poll_until(&lsp, "the server started again", || record.launches() == 2);
}

#[test]
fn removed_watch_root_requests_backend_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir(&root).unwrap();
    let watcher = start(&root, None);
    let receiver = watcher.receiver();
    std::fs::remove_dir_all(&root).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if receiver.take().error.is_some() {
            assert!(!watcher.is_healthy());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "removed root was not observed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A watcher over a fresh repository at `dir`.
fn start(dir: &Path, files: Option<FileEvents>) -> ChangeWatcher {
    init_repo(dir);
    let repo = git::Repo::discover_at(dir).unwrap();
    ChangeWatcher::start(&repo, files).unwrap().unwrap()
}
