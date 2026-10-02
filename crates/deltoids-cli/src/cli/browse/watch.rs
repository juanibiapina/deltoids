//! Bounded filesystem notifications and Git-aware refresh filtering.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use notify::{RecursiveMode, Watcher};

use deltoids::git;

const MAX_PENDING_PATHS: usize = 4096;

/// One coalesced batch. A rescan replaces paths when the backend or this
/// accumulator loses detail. Errors require reinstalling the watcher.
#[derive(Default)]
pub(crate) struct ChangeBatch {
    pub paths: HashSet<PathBuf>,
    pub rescan: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct PendingChanges {
    batch: Mutex<ChangeBatch>,
    failed: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct ChangeReceiver(Arc<PendingChanges>);

impl ChangeReceiver {
    pub fn request_rescan(&self) {
        let mut batch = self.0.batch.lock().unwrap_or_else(|err| err.into_inner());
        batch.paths.clear();
        batch.rescan = true;
    }

    /// Take pending work atomically; later callbacks belong to the next batch.
    pub fn take(&self) -> ChangeBatch {
        std::mem::take(&mut *self.0.batch.lock().unwrap_or_else(|err| err.into_inner()))
    }
}

struct ChangeSender(Arc<PendingChanges>);

impl ChangeSender {
    fn send(&self, result: notify::Result<notify::Event>) {
        let mut pending = self.0.batch.lock().unwrap_or_else(|err| err.into_inner());
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                self.0.failed.store(true, Ordering::Relaxed);
                pending.error.get_or_insert_with(|| err.to_string());
                pending.rescan = true;
                pending.paths.clear();
                return;
            }
        };
        // Linux reports file opens, including reads performed by our own diff.
        // Only a write-close can represent new content among access events.
        if matches!(event.kind, notify::EventKind::Access(kind)
            if !matches!(kind, notify::event::AccessKind::Close(notify::event::AccessMode::Write)))
            && !event.need_rescan()
        {
            return;
        }
        if event.need_rescan() || event.paths.is_empty() {
            pending.rescan = true;
            pending.paths.clear();
        }
        if pending.rescan {
            return;
        }
        for path in event.paths {
            pending.paths.insert(path);
            if pending.paths.len() > MAX_PENDING_PATHS {
                pending.paths.clear();
                pending.rescan = true;
                break;
            }
        }
    }
}

impl Drop for ChangeSender {
    fn drop(&mut self) {
        self.send(Err(notify::Error::generic(
            "filesystem watcher disconnected",
        )));
    }
}

fn change_channel() -> (ChangeSender, ChangeReceiver) {
    let pending = Arc::new(PendingChanges::default());
    (ChangeSender(pending.clone()), ChangeReceiver(pending))
}

fn normalize_event(
    mut event: notify::Event,
    roots: &[(PathBuf, PathBuf)],
) -> notify::Result<notify::Event> {
    let removed = matches!(
        event.kind,
        notify::EventKind::Remove(_)
            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
    );
    if removed
        && event.paths.iter().any(|path| {
            roots
                .iter()
                .any(|(canonical, original)| path == canonical || path == original)
        })
    {
        return Err(notify::Error::generic("watched directory moved or removed"));
    }
    // FSEvents resolves symlinked roots; callers filter against the original
    // repository paths (notably /var on macOS).
    for path in &mut event.paths {
        if let Some(mapped) = roots.iter().find_map(|(canonical, original)| {
            path.strip_prefix(canonical)
                .ok()
                .map(|relative| original.join(relative))
        }) {
            *path = mapped;
        }
    }
    Ok(event)
}

/// Owns the backend and its bounded pending batch. Taking notifications
/// never blocks on filesystem I/O; callbacks never queue an unbounded backlog.
pub(super) struct ChangeWatcher {
    _backend: notify::RecommendedWatcher,
    receiver: ChangeReceiver,
}

impl ChangeWatcher {
    pub fn new(roots: &[&Path]) -> Result<Self, String> {
        let (sender, receiver) = change_channel();
        let roots: Vec<_> = roots
            .iter()
            .map(|root| {
                root.canonicalize()
                    .map(|canonical| (canonical, root.to_path_buf()))
                    .map_err(|err| format!("failed to resolve {}: {err}", root.display()))
            })
            .collect::<Result<_, _>>()?;
        let callback_roots = roots.clone();
        let mut backend =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                sender.send(result.and_then(|event| normalize_event(event, &callback_roots)));
            })
            .map_err(|err| format!("failed to create filesystem watcher: {err}"))?;
        for (_, root) in &roots {
            backend
                .watch(root, RecursiveMode::Recursive)
                .map_err(|err| format!("failed to watch {}: {err}", root.display()))?;
        }
        Ok(Self {
            _backend: backend,
            receiver,
        })
    }

    pub fn is_healthy(&self) -> bool {
        !self.receiver.0.failed.load(Ordering::Relaxed)
    }

    pub fn receiver(&self) -> ChangeReceiver {
        self.receiver.clone()
    }
}

/// Watch the workdir and Git metadata, including shared refs outside a
/// linked worktree. Bare repositories have no working-tree watcher.
pub(super) fn spawn_workdir_watcher(repo: &git::Repo) -> Result<Option<ChangeWatcher>, String> {
    let Some(workdir) = repo.workdir() else {
        return Ok(None);
    };
    let mut roots = vec![workdir];
    // The recursive workdir watch already covers an ordinary .git directory.
    for root in [repo.git_dir(), repo.common_dir()] {
        if !roots.iter().any(|parent| root.starts_with(parent)) {
            roots.push(root);
        }
    }
    ChangeWatcher::new(&roots).map(Some)
}

/// Git control files can change the diff or its staging status. Ignore
/// locks, objects, and reflogs, whose churn cannot change the review.
fn is_git_control_path(relative: &Path) -> bool {
    let first = relative.components().next().map(|part| part.as_os_str());
    match first.and_then(|part| part.to_str()) {
        None => true,
        Some("HEAD" | "index" | "packed-refs" | "config" | "config.worktree" | "shallow") => true,
        Some("refs") => !relative.to_string_lossy().ends_with(".lock"),
        Some("info") => matches!(relative.to_str(), Some("info/exclude" | "info/attributes")),
        _ => false,
    }
}

/// Relevant workdir edits or Git metadata changes warrant a refresh.
pub(super) fn path_warrants_reload(repo: &git::Repo, paths: &[PathBuf]) -> bool {
    paths.iter().any(|path| {
        for root in [repo.git_dir(), repo.common_dir()] {
            if let Ok(relative) = path.strip_prefix(root) {
                return is_git_control_path(relative);
            }
        }
        !is_git_internal(path) && !repo.is_ignored(path)
    })
}

pub(super) fn is_git_internal(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Init a repo at `dir` with a committable identity configured.
    fn init_repo(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        repo
    }

    fn stage_all(repo: &git2::Repository) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
    }

    fn commit_index(repo: &git2::Repository, msg: &str) {
        let mut index = repo.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap();
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
        let (sender, receiver) = change_channel();
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
        let (sender, receiver) = change_channel();
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
        let (sender, receiver) = change_channel();
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
        let watcher = ChangeWatcher::new(&[&root]).unwrap();
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
        let (sender, receiver) = change_channel();
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
        let watcher = spawn_workdir_watcher(&repo).unwrap().unwrap();
        git_repo
            .config()
            .unwrap()
            .set_bool("core.filemode", false)
            .unwrap();
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
        let _worktree = git_repo.worktree("linked", &linked, None).unwrap();
        let repo = git::Repo::discover_at(&linked).unwrap();
        let watcher = spawn_workdir_watcher(&repo).unwrap().unwrap();
        let oid = git_repo.head().unwrap().target().unwrap();
        git_repo
            .reference("refs/heads/another", oid, true, "probe")
            .unwrap();
        wait_for_relevant_change(&repo, &watcher.receiver());
    }
    #[test]
    fn removed_watch_root_requests_backend_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("watched");
        std::fs::create_dir(&root).unwrap();
        let watcher = ChangeWatcher::new(&[&root]).unwrap();
        let receiver = watcher.receiver();
        std::fs::remove_dir(&root).unwrap();
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
}
