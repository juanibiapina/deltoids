//! The bounded batch between the backend's callback and the TUI.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::lsp::FileEvents;

pub(super) const MAX_PENDING_PATHS: usize = 4096;

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

impl PendingChanges {
    fn lock(&self) -> std::sync::MutexGuard<'_, ChangeBatch> {
        self.batch.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn fail(&self, error: String) {
        self.failed.store(true, Ordering::Relaxed);
        let mut pending = self.lock();
        pending.error.get_or_insert(error);
        pending.rescan = true;
        pending.paths.clear();
    }
}

#[derive(Clone)]
pub(crate) struct ChangeReceiver(Arc<PendingChanges>);

impl ChangeReceiver {
    pub fn request_rescan(&self) {
        let mut batch = self.0.lock();
        batch.paths.clear();
        batch.rescan = true;
    }

    /// Take pending work atomically; later callbacks belong to the next batch.
    pub fn take(&self) -> ChangeBatch {
        std::mem::take(&mut *self.0.lock())
    }

    /// Report an error from outside the backend's callback. The watcher
    /// is unhealthy from then on.
    #[cfg(target_os = "linux")]
    pub(super) fn fail(&self, error: String) {
        self.0.fail(error);
    }

    pub(super) fn is_failed(&self) -> bool {
        self.0.failed.load(Ordering::Relaxed)
    }
}

pub(super) struct ChangeSender {
    pending: Arc<PendingChanges>,
    /// The language servers' files: every event, ahead of the bounded
    /// batch and its filter, so an overflow never hides an edit from
    /// them. Each consumer decides what counts as a change.
    files: Option<FileEvents>,
}

impl ChangeSender {
    pub(super) fn send(&self, result: notify::Result<notify::Event>) {
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                if let Some(files) = &self.files {
                    files.lost();
                }
                self.pending.fail(err.to_string());
                return;
            }
        };
        if let Some(files) = &self.files {
            files.record(&event);
        }
        // Linux reports file opens, including reads performed by our own diff.
        // Only a write-close can represent new content among access events.
        if matches!(event.kind, notify::EventKind::Access(kind)
            if !matches!(kind, notify::event::AccessKind::Close(notify::event::AccessMode::Write)))
            && !event.need_rescan()
        {
            return;
        }
        let mut pending = self.pending.lock();
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

pub(super) fn change_channel(files: Option<FileEvents>) -> (ChangeSender, ChangeReceiver) {
    let pending = Arc::new(PendingChanges::default());
    let sender = ChangeSender {
        pending: pending.clone(),
        files,
    };
    (sender, ChangeReceiver(pending))
}
