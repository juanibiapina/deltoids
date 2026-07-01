//! Shared working-tree watcher: install a recursive filesystem watcher
//! on a repo's workdir and decide when a batch of changed paths warrants
//! a reload.
//!
//! [`super::files::FilesMode`] watches the repo working tree and filters
//! the noise (`.git/` churn and gitignored files), so the essence lives
//! here once. This is a "replace, don't layer" extraction: the mode
//! delegates to these functions rather than inlining the watcher/filter
//! logic.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use deltoids::git;

/// Install a recursive filesystem watcher on `repo`'s working directory.
///
/// The callback only forwards each event's paths over the returned
/// channel: git2's `Repository` is `!Send` and can't move into the (Send)
/// closure, so gitignore filtering stays on the main thread. For a bare
/// repo (no workdir) no watcher is created and the receiver never fires
/// (the sender is dropped here).
#[allow(clippy::type_complexity)]
pub(super) fn spawn_workdir_watcher(
    repo: &git::Repo,
) -> Result<
    (
        Option<notify::RecommendedWatcher>,
        mpsc::Receiver<Vec<PathBuf>>,
    ),
    String,
> {
    let (notify_tx, notify_rx) = mpsc::channel::<Vec<PathBuf>>();
    let watcher = match repo.workdir() {
        Some(workdir) => {
            let mut watcher =
                notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    if let Ok(event) = res {
                        let _ = notify_tx.send(event.paths);
                    }
                })
                .map_err(|err| format!("failed to create filesystem watcher: {err}"))?;
            watcher
                .watch(workdir, RecursiveMode::Recursive)
                .map_err(|err| format!("failed to watch {}: {err}", workdir.display()))?;
            Some(watcher)
        }
        None => None,
    };
    Ok((watcher, notify_rx))
}

/// Whether a batch of changed `paths` warrants a working-tree reload.
///
/// A path counts when it is neither inside `.git/` (git's constant
/// index/lock churn) nor gitignored (ignored files never appear in
/// `working_tree_diff`, so a change there can't alter the diff). Fails
/// open via [`git::Repo::is_ignored`], so a real change is never missed.
pub(super) fn path_warrants_reload(repo: &git::Repo, paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|path| !is_git_internal(path) && !repo.is_ignored(path))
}

/// Whether `path` lies inside a `.git` directory.
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
                dir.path().join(".git/index"),
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
}
