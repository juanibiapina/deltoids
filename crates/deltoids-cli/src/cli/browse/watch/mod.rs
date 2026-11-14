//! Filesystem notifications for a repository's working tree and Git
//! metadata, delivered as bounded batches, plus the filter that decides
//! which changed paths warrant a reload.
//!
//! On Linux, inotify costs one watch per directory, so the watcher skips
//! gitignored top-level directories (`target/`, `.direnv/`) and never
//! follows symlinks out of the repository. Elsewhere it watches the tree
//! recursively.

use std::path::{Path, PathBuf};

use notify::{Config, RecommendedWatcher, Watcher};

use deltoids::git;

use crate::lsp::FileEvents;

mod batch;
#[cfg(not(target_os = "linux"))]
mod layout;
#[cfg(target_os = "linux")]
#[path = "layout_linux.rs"]
mod layout;
#[cfg(test)]
mod tests;

pub(crate) use batch::ChangeReceiver;
#[cfg(test)]
use batch::MAX_PENDING_PATHS;
use batch::change_channel;

/// Watches a repository's working tree and Git metadata, including shared
/// refs outside a linked worktree. Every change that can alter the diff
/// reaches the receiver as a path or a rescan; `files` hears about every
/// event. Taking a batch never blocks on filesystem I/O, and callbacks
/// never queue an unbounded backlog. A watcher that reports an error is
/// unhealthy and must be started again.
pub(super) struct ChangeWatcher {
    _layout: layout::Layout,
    receiver: ChangeReceiver,
}

impl ChangeWatcher {
    /// Start watching `repo`. `None` for a bare repository, which has no
    /// working tree.
    pub fn start(repo: &git::Repo, files: Option<FileEvents>) -> Result<Option<Self>, String> {
        let Some(workdir) = repo.workdir() else {
            return Ok(None);
        };
        // The workdir watch already covers an ordinary .git directory.
        let mut git_roots: Vec<PathBuf> = Vec::new();
        for root in [repo.git_dir(), repo.common_dir()] {
            if !root.starts_with(workdir) && !git_roots.iter().any(|known| root.starts_with(known))
            {
                git_roots.push(root.to_path_buf());
            }
        }
        let roots = std::iter::once(workdir)
            .chain(git_roots.iter().map(PathBuf::as_path))
            .map(|root| {
                root.canonicalize()
                    .map(|canonical| (canonical, root.to_path_buf()))
                    .map_err(|err| format!("failed to resolve {}: {err}", root.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (sender, receiver) = change_channel(files);
        let backend = move |tap: layout::Tap| {
            RecommendedWatcher::new(
                handle_events(roots, tap, sender),
                Config::default().with_follow_symlinks(false),
            )
            .map_err(|err| format!("failed to create filesystem watcher: {err}"))
        };
        let layout = layout::Layout::start(workdir, &git_roots, repo, receiver.clone(), backend)?;
        Ok(Some(Self {
            _layout: layout,
            receiver,
        }))
    }

    pub fn is_healthy(&self) -> bool {
        !self.receiver.is_failed()
    }

    pub fn receiver(&self) -> ChangeReceiver {
        self.receiver.clone()
    }
}

/// The backend's callback: maps events back to the `roots` callers
/// named, shows each to the layout's `tap`, and batches it.
fn handle_events(
    roots: Vec<(PathBuf, PathBuf)>,
    tap: layout::Tap,
    sender: batch::ChangeSender,
) -> impl FnMut(notify::Result<notify::Event>) {
    move |result| {
        let result = result.and_then(|event| normalize_event(event, &roots));
        if let Ok(event) = &result {
            tap.see(event);
        }
        sender.send(result);
    }
}

/// The error for a failed watch of `path`.
fn watch_error(path: &Path, err: notify::Error) -> String {
    format!("failed to watch {}: {err}", path.display())
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
    let mut ignores = repo.ignores();
    paths.iter().any(|path| {
        for root in [repo.git_dir(), repo.common_dir()] {
            if let Ok(relative) = path.strip_prefix(root) {
                return is_git_control_path(relative);
            }
        }
        !is_git_internal(path)
            && !ignores
                .as_mut()
                .is_some_and(|ignores| ignores.is_ignored(path))
    })
}

pub(super) fn is_git_internal(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}
