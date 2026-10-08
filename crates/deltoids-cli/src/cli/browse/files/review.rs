//! Review guidance for the Files sidebar: the Jev judgments gathered
//! this session, the background job that asks Jev about new hunks, and
//! what the sidebar, footer, and hunk headers draw from them.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::judgments::{
    self, Attention, ChangeSet, ChangeStatus, ChangedFile, FileNote, JevRequest, Judgments,
    SendError, jev,
};
use crate::sidebar::{FileStatus, RowNote, display_path, file_status};

use super::model::{Column, FileBody, Model};

pub(super) type Sender = Arc<dyn Fn(&JevRequest) -> Result<String, SendError> + Send + Sync>;

type Replies = Vec<(JevRequest, Result<String, String>)>;

#[derive(Default)]
pub(super) struct Review {
    judgments: Judgments,
    sender: Option<Sender>,
    job: Option<Receiver<Replies>>,
    /// Bumped on new judgments so hunk labels redraw.
    revision: u64,
}

#[derive(Default)]
pub(super) struct Guidance {
    pub(super) rows: Vec<Option<RowNote>>,
    pub(super) low: Vec<bool>,
}

impl Review {
    pub(super) fn from_env() -> Self {
        let sender = jev::key_from_env().map(|key| -> Sender {
            Arc::new(move |request: &JevRequest| jev::send(&key, request))
        });
        Self {
            sender,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn with_sender(sender: Sender) -> Self {
        Self {
            sender: Some(sender),
            ..Self::default()
        }
    }

    pub(super) fn guidance(&self, model: &Model) -> Guidance {
        if self.sender.is_none() {
            return Guidance::default();
        }
        let notes = judgments::file_notes(&change_set(model), &self.judgments);
        Guidance {
            low: notes.iter().map(|note| note.low).collect(),
            rows: notes.into_iter().map(row_note).collect(),
        }
    }

    pub(super) fn request(&mut self, model: &Model) {
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let requests = judgments::pending_requests(&change_set(model), &self.judgments);
        if requests.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let send = |request: &JevRequest| sender(request);
            let _ = tx.send(judgments::send_all(&send, requests));
        });
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn hunk_label(
        &self,
        model: &Model,
        column: Column,
        file: usize,
        hunk: usize,
    ) -> Option<RowNote> {
        if file >= model.files.len() {
            return None;
        }
        let view = model.view(file, column);
        let FileBody::Diff(diff) = view.body else {
            return None;
        };
        let judgment = self
            .judgments
            .of(display_path(view.file), diff.hunks().get(hunk)?)?;
        row_note(judgment.note())
    }

    pub(super) fn pending(&self) -> bool {
        self.job.is_some()
    }

    /// `None` until the job finishes; then the first error, if any.
    pub(super) fn collect(&mut self) -> Option<Result<(), String>> {
        let receiver = self.job.as_ref()?;
        let replies = match receiver.try_recv() {
            Ok(replies) => replies,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        self.job = None;
        let mut first_error = None;
        for (request, reply) in replies {
            let absorbed = reply.and_then(|body| {
                judgments::absorb(&mut self.judgments, &request, &body)
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = absorbed {
                first_error.get_or_insert(error);
            }
        }
        self.revision += 1;
        Some(first_error.map_or(Ok(()), Err))
    }
}

fn row_note(note: FileNote) -> Option<RowNote> {
    let level = match note.attention {
        None | Some(Attention::Straightforward) => 0,
        Some(Attention::Careful) => 1,
        Some(Attention::Critical) => 2,
    };
    Some(RowNote {
        tag: note.tag?,
        level,
        low: note.low,
        breaking: note.breaking,
    })
}

fn change_set(model: &Model) -> ChangeSet {
    ChangeSet {
        files: model
            .files
            .iter()
            .zip(&model.bodies)
            .map(|(resolved, body)| {
                let path = display_path(&resolved.file);
                let status = change_status(file_status(&resolved.file));
                match body {
                    FileBody::Diff(diff) => ChangedFile::from_diff(path, status, diff),
                    _ => ChangedFile {
                        path: path.to_string(),
                        status,
                        hunks: Vec::new(),
                    },
                }
            })
            .collect(),
    }
}

fn change_status(status: FileStatus) -> ChangeStatus {
    match status {
        FileStatus::Added => ChangeStatus::Added,
        FileStatus::Deleted => ChangeStatus::Deleted,
        FileStatus::Renamed | FileStatus::Copied => ChangeStatus::Renamed,
        FileStatus::Modified | FileStatus::TypeChanged => ChangeStatus::Modified,
    }
}
