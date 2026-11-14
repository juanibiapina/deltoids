//! inotify costs one watch per directory, and a recursive watch walks the
//! whole tree. This layout watches the working tree's root by itself and
//! each top-level directory that git does not ignore recursively, so
//! trees such as `target/` and `.direnv/` cost nothing. A thread keeps
//! that set current as top-level entries and ignore rules change: the
//! backend cannot change its watches from inside its own callback.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, bounded, select};
use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{ErrorKind, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use deltoids::git;

use super::batch::ChangeReceiver;
use super::watch_error;

/// Keeps the watches current until dropped.
pub(super) struct Layout {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Layout {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Top-level paths whose watch may no longer match the directory there.
#[derive(Default)]
struct Dirty {
    paths: Mutex<HashSet<PathBuf>>,
    /// The backend lost events: any top-level watch may be stale.
    all: AtomicBool,
}

impl Dirty {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<PathBuf>> {
        self.paths.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// Sees every event in the backend's callback and wakes the layout thread
/// when the set of top-level directories or the ignore rules may have
/// changed. Never blocks.
pub(super) struct Tap {
    workdir: PathBuf,
    /// Files whose edits can change which directories git ignores.
    rules: Vec<PathBuf>,
    dirty: Arc<Dirty>,
    wake: Sender<()>,
}

impl Tap {
    pub fn see(&self, event: &notify::Event) {
        if event.need_rescan() {
            self.dirty.all.store(true, Ordering::Relaxed);
            self.wake();
            return;
        }
        let moved = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        let read = matches!(event.kind, EventKind::Access(kind)
            if kind != AccessKind::Close(AccessMode::Write));
        let mut wake = false;
        for path in &event.paths {
            if moved && path.parent() == Some(self.workdir.as_path()) {
                self.dirty.lock().insert(path.clone());
                wake = true;
            } else if !read && self.rules.contains(path) {
                wake = true;
            }
        }
        if wake {
            self.wake();
        }
    }

    fn wake(&self) {
        let _ = self.wake.try_send(());
    }
}

impl Layout {
    /// Watch `workdir` and its unignored top-level directories, and
    /// `git_roots` recursively, with the backend `backend` builds.
    /// Reports later failures through `receiver`.
    pub fn start(
        workdir: &Path,
        git_roots: &[PathBuf],
        repo: &git::Repo,
        receiver: ChangeReceiver,
        backend: impl FnOnce(Tap) -> Result<RecommendedWatcher, String>,
    ) -> Result<Self, String> {
        let (wake, woken) = bounded(1);
        let dirty = Arc::new(Dirty::default());
        let tap = Tap {
            workdir: workdir.to_path_buf(),
            rules: vec![
                workdir.join(".gitignore"),
                repo.git_dir().join("info/exclude"),
                repo.common_dir().join("info/exclude"),
                repo.git_dir().join("index"),
            ],
            dirty: dirty.clone(),
            wake: wake.clone(),
        };
        let mut backend = backend(tap)?;
        backend
            .watch(workdir, RecursiveMode::NonRecursive)
            .map_err(|err| watch_error(workdir, err))?;
        for root in git_roots {
            backend
                .watch(root, RecursiveMode::Recursive)
                .map_err(|err| watch_error(root, err))?;
        }
        let mut tree = TopLevel {
            backend,
            workdir: workdir.to_path_buf(),
            watched: BTreeSet::new(),
            dirty,
            retry: false,
        };
        tree.reconcile(repo)?;
        let (stop, stopped) = bounded(0);
        let thread = std::thread::Builder::new()
            .name("deltoids-watch".into())
            .spawn(move || tree.run(&woken, &stopped, &wake, &receiver))
            .map_err(|err| format!("failed to start the watcher thread: {err}"))?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

/// The backend and the top-level directories it watches recursively.
struct TopLevel {
    backend: RecommendedWatcher,
    workdir: PathBuf,
    watched: BTreeSet<PathBuf>,
    dirty: Arc<Dirty>,
    /// A directory vanished while its watch was being added but its path
    /// exists again: reconcile once more.
    retry: bool,
}

impl TopLevel {
    fn run(
        mut self,
        woken: &Receiver<()>,
        stopped: &Receiver<()>,
        wake: &Sender<()>,
        receiver: &ChangeReceiver,
    ) {
        let Some(repo) = git::Repo::discover_at(&self.workdir) else {
            receiver.fail(format!("failed to open {}", self.workdir.display()));
            return;
        };
        loop {
            select! {
                recv(stopped) -> _ => return,
                recv(woken) -> _ => {}
            }
            match self.reconcile(&repo) {
                Ok(true) => receiver.request_rescan(),
                Ok(false) => {}
                Err(err) => {
                    receiver.fail(err);
                    return;
                }
            }
            if std::mem::take(&mut self.retry) {
                let _ = wake.try_send(());
            }
        }
    }

    /// Make the watched set match the unignored top-level directories.
    /// Stale watches go first: after a rename the old and new paths share
    /// one inotify watch, so removing the old one later would remove both.
    /// Returns whether the set changed, which may have missed events.
    fn reconcile(&mut self, repo: &git::Repo) -> Result<bool, String> {
        let mut changed = false;
        let dirty: Vec<PathBuf> = if self.dirty.all.swap(false, Ordering::Relaxed) {
            self.dirty.lock().clear();
            self.watched.iter().cloned().collect()
        } else {
            self.dirty.lock().drain().collect()
        };
        for path in dirty {
            if self.watched.remove(&path) {
                self.unwatch(&path)?;
                changed = true;
            }
        }
        let wanted = unignored_dirs(&self.workdir, repo)?;
        let stale: Vec<PathBuf> = self.watched.difference(&wanted).cloned().collect();
        for path in stale {
            self.watched.remove(&path);
            self.unwatch(&path)?;
            changed = true;
        }
        for path in wanted {
            if self.watched.contains(&path) {
                continue;
            }
            if self.watch(&path)? {
                self.watched.insert(path);
                changed = true;
            } else if path.is_dir() {
                self.retry = true;
            }
        }
        Ok(changed)
    }

    /// Watch `path` recursively. `false` when it vanished on the way.
    fn watch(&mut self, path: &Path) -> Result<bool, String> {
        match self.backend.watch(path, RecursiveMode::Recursive) {
            Ok(()) => Ok(true),
            Err(err) if is_gone(&err) => {
                // A subdirectory vanished mid-walk: drop what was added.
                let _ = self.backend.unwatch(path);
                Ok(false)
            }
            Err(err) => Err(watch_error(path, err)),
        }
    }

    fn unwatch(&mut self, path: &Path) -> Result<(), String> {
        match self.backend.unwatch(path) {
            Ok(()) => Ok(()),
            Err(err) if is_gone(&err) => Ok(()),
            Err(err) => Err(format!("failed to unwatch {}: {err}", path.display())),
        }
    }
}

/// The backend dropped the watch itself, or the path no longer exists.
fn is_gone(err: &notify::Error) -> bool {
    match &err.kind {
        ErrorKind::WatchNotFound | ErrorKind::PathNotFound => true,
        ErrorKind::Io(io) => io.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

/// The directories directly in `workdir` that git does not ignore.
/// Symlinks are not directories here.
fn unignored_dirs(workdir: &Path, repo: &git::Repo) -> Result<BTreeSet<PathBuf>, String> {
    let entries = std::fs::read_dir(workdir)
        .map_err(|err| format!("failed to read {}: {err}", workdir.display()))?;
    let mut ignores = repo.ignores();
    Ok(entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            !ignores
                .as_mut()
                .is_some_and(|ignores| ignores.is_ignored(path))
        })
        .collect())
}
