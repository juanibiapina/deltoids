//! Watches the working tree and Git directories recursively. Backends
//! other than inotify watch a tree without a cost per directory.

use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use deltoids::git;

use super::batch::ChangeReceiver;
use super::watch_error;

/// Keeps the watches alive.
pub(super) struct Layout {
    _backend: RecommendedWatcher,
}

/// Sees every event before the batch does. This layout needs none.
pub(super) struct Tap;

impl Tap {
    pub fn see(&self, _event: &notify::Event) {}
}

impl Layout {
    /// Watch `workdir` and `git_roots` recursively with the backend
    /// `backend` builds.
    pub fn start(
        workdir: &Path,
        git_roots: &[PathBuf],
        _repo: &git::Repo,
        _receiver: ChangeReceiver,
        backend: impl FnOnce(Tap) -> Result<RecommendedWatcher, String>,
    ) -> Result<Self, String> {
        let mut backend = backend(Tap)?;
        for root in std::iter::once(workdir).chain(git_roots.iter().map(PathBuf::as_path)) {
            backend
                .watch(root, RecursiveMode::Recursive)
                .map_err(|err| watch_error(root, err))?;
        }
        Ok(Self { _backend: backend })
    }
}
