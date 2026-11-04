//! Review guidance for the Files sidebar: the Jev judgments gathered
//! this session, the background job that asks Jev about new hunks, and
//! what the sidebar, footer, and hunk headers draw from them.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use deltoids_jev::{
    Answers, Attention, ChangeStatus, ChangedFile, ChangedHunk, Judgments, Note, Post, Reply,
};

use crate::jev;
use crate::sidebar::{FileStatus, RowNote, display_path, file_status};

use super::model::{Column, FileBody, Model};

/// The POST that reaches Jev, with the key it sends.
pub(super) type Sender = Arc<dyn Fn(Post<'_>) -> Result<Reply, String> + Send + Sync>;

#[derive(Default)]
pub(super) struct Review {
    judgments: Judgments,
    sender: Option<(String, Sender)>,
    job: Option<Receiver<Answers>>,
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
        let sender =
            jev::key_from_env().map(|key| -> (String, Sender) { (key, Arc::new(jev::post)) });
        Self {
            sender,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn with_sender(sender: Sender) -> Self {
        Self {
            sender: Some(("test-key".to_string(), sender)),
            ..Self::default()
        }
    }

    pub(super) fn guidance(&self, model: &Model) -> Guidance {
        if self.sender.is_none() {
            return Guidance::default();
        }
        let notes = self.judgments.file_notes(&change_set(model));
        Guidance {
            low: notes
                .iter()
                .map(|note| note.is_some_and(|note| note.low))
                .collect(),
            rows: notes.into_iter().map(|note| note.map(row_note)).collect(),
        }
    }

    pub(super) fn request(&mut self, model: &Model) {
        let Some((key, sender)) = self.sender.clone() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let Some(questions) = self.judgments.pending(&change_set(model)) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let post = |post: Post<'_>| sender(post);
            let _ = tx.send(questions.ask(&key, &post));
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
        let hunk = ChangedHunk::from_hunk(diff.hunks().get(hunk)?);
        let note = self.judgments.note(display_path(view.file), &hunk)?;
        Some(row_note(note))
    }

    pub(super) fn pending(&self) -> bool {
        self.job.is_some()
    }

    /// `None` until the job finishes; then the first error, if any.
    pub(super) fn collect(&mut self) -> Option<Result<(), String>> {
        let receiver = self.job.as_ref()?;
        let answers = match receiver.try_recv() {
            Ok(answers) => Some(answers),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        self.job = None;
        let result = answers.map_or(Ok(()), |answers| self.judgments.add(answers));
        self.revision += 1;
        Some(result)
    }
}

fn row_note(note: Note) -> RowNote {
    let level = match note.attention {
        None | Some(Attention::Straightforward) => 0,
        Some(Attention::Careful) => 1,
        Some(Attention::Critical) => 2,
    };
    RowNote {
        tag: note.tag,
        level,
        low: note.low,
    }
}

fn change_set(model: &Model) -> Vec<ChangedFile> {
    model
        .files
        .iter()
        .zip(&model.bodies)
        .take(model.changed_files())
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
        .collect()
}

fn change_status(status: FileStatus) -> ChangeStatus {
    match status {
        FileStatus::Added => ChangeStatus::Added,
        FileStatus::Deleted => ChangeStatus::Deleted,
        FileStatus::Renamed | FileStatus::Copied => ChangeStatus::Renamed,
        FileStatus::Modified | FileStatus::TypeChanged => ChangeStatus::Modified,
    }
}
