//! Jev judgments of a change: ask Jev what role each hunk plays and how
//! much attention it needs, and summarize the answers per file.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use deltoids::LineKind;
use serde_json::{Map, Value};

pub(crate) mod jev;
mod paths;
mod request;

use paths::{is_lockfile_path, never_sent};
pub(crate) use request::pending_requests;
#[cfg(test)]
pub(crate) use request::{STATE_BUDGET_CHARS, TOTAL_BUDGET_CHARS};

pub(crate) struct ChangeSet {
    pub(crate) files: Vec<ChangedFile>,
}

pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) status: ChangeStatus,
    pub(crate) hunks: Vec<ChangedHunk>,
}

impl ChangedFile {
    pub(crate) fn from_diff(path: &str, status: ChangeStatus, diff: &deltoids::Diff) -> Self {
        let hunks = diff.hunks().iter().map(ChangedHunk::from_hunk).collect();
        Self {
            path: path.to_string(),
            status,
            hunks,
        }
    }
}

fn lines_of(hunk: &deltoids::Hunk, kind: LineKind) -> Vec<String> {
    hunk.lines
        .iter()
        .filter(|line| line.kind == kind)
        .map(|line| line.content.trim_end_matches(['\n', '\r']).to_string())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

pub(crate) struct ChangedHunk {
    pub(crate) scope: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) added: Vec<String>,
}

impl ChangedHunk {
    fn from_hunk(hunk: &deltoids::Hunk) -> Self {
        Self {
            scope: hunk
                .ancestors
                .iter()
                .filter(|node| !node.name.is_empty())
                .map(|node| node.name.clone())
                .collect(),
            removed: lines_of(hunk, LineKind::Removed),
            added: lines_of(hunk, LineKind::Added),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Core,
    Refactor,
    Config,
    Deps,
    Build,
    Ci,
    Removal,
    Test,
    Docs,
    Comments,
    Imports,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Refactor => "refactor",
            Self::Config => "config",
            Self::Deps => "deps",
            Self::Build => "build",
            Self::Ci => "ci",
            Self::Removal => "removal",
            Self::Test => "test",
            Self::Docs => "docs",
            Self::Comments => "comments",
            Self::Imports => "imports",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "core" => Self::Core,
            "refactor" => Self::Refactor,
            "config" => Self::Config,
            "deps" => Self::Deps,
            "build" => Self::Build,
            "ci" => Self::Ci,
            "removal" => Self::Removal,
            "test" => Self::Test,
            "docs" => Self::Docs,
            "comments" => Self::Comments,
            "imports" => Self::Imports,
            _ => return None,
        })
    }
}

/// A hunk breaks callers when Jev's yes probability reaches this. On this
/// repo's history, 0.6 caught removed CLI commands and public library
/// functions and flagged none of 214 hunks of ordinary feature or fix
/// commits; 0.5 also caught crate-internal changes.
const BREAKING_THRESHOLD: f64 = 0.6;

/// `attention` runs from 0 (straightforward) to 2 (critical); `breaking` is the
/// probability that the hunk breaks code or users outside its package.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Judgment {
    pub(crate) role: Role,
    pub(crate) role_confidence: f64,
    pub(crate) attention: f64,
    pub(crate) breaking: f64,
}

impl Judgment {
    pub(crate) fn note(&self) -> FileNote {
        let tag = self.role.name();
        FileNote {
            tag: Some(tag),
            attention: Some(Attention::of(self.attention)),
            low: matches!(tag, "test" | "comments" | "imports"),
            breaking: self.breaking >= BREAKING_THRESHOLD,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Attention {
    Straightforward,
    Careful,
    Critical,
}

impl Attention {
    fn of(score: f64) -> Self {
        match score {
            score if score < 0.5 => Self::Straightforward,
            score if score < 1.5 => Self::Careful,
            _ => Self::Critical,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FileNote {
    pub(crate) tag: Option<&'static str>,
    pub(crate) attention: Option<Attention>,
    pub(crate) low: bool,
    pub(crate) breaking: bool,
}

#[derive(Default)]
pub(crate) struct Judgments(HashMap<HunkKey, Judgment>);

impl Judgments {
    pub(crate) fn of(&self, path: &str, hunk: &deltoids::Hunk) -> Option<&Judgment> {
        self.0
            .get(&HunkKey::of(path, &ChangedHunk::from_hunk(hunk)))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AbsorbError {
    Malformed,
    Missing(usize),
}

impl std::fmt::Display for AbsorbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "unreadable Jev response"),
            Self::Missing(count) => write!(f, "Jev left {count} hunks unanswered"),
        }
    }
}

/// Records every valid answer, even when it returns an error.
pub(crate) fn absorb(
    known: &mut Judgments,
    request: &JevRequest,
    response: &str,
) -> Result<(), AbsorbError> {
    let response: Value = serde_json::from_str(response).map_err(|_| AbsorbError::Malformed)?;
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(AbsorbError::Malformed)?;
    let mut missing = 0;
    for (n, key) in request.hunks.iter().enumerate() {
        match judgment(answers, &format!("h{n}")) {
            Some(judgment) => {
                known.0.insert(*key, judgment);
            }
            None => missing += 1,
        }
    }
    if missing > 0 {
        return Err(AbsorbError::Missing(missing));
    }
    Ok(())
}

fn judgment(answers: &Map<String, Value>, id: &str) -> Option<Judgment> {
    let role = answers.get(&format!("role_{id}"))?;
    let attention = answers.get(&format!("attention_{id}"))?;
    let breaking = answers.get(&format!("breaking_{id}"))?;
    Some(Judgment {
        role: Role::parse(role.get("choice")?.as_str()?)?,
        role_confidence: role.get("confidence")?.as_f64()?,
        attention: attention.get("score")?.as_f64()?,
        breaking: breaking.get("noul")?.as_f64()?,
    })
}

/// Path plus changed lines, so it survives line shifts and reloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HunkKey(u64);

impl HunkKey {
    fn of(path: &str, hunk: &ChangedHunk) -> Self {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        hunk.removed.hash(&mut hasher);
        hunk.added.hash(&mut hasher);
        Self(hasher.finish())
    }
}

pub(crate) struct JevRequest {
    pub(crate) body: String,
    pub(crate) hunks: Vec<HunkKey>,
    parts: request::Parts,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SendError {
    TooLarge,
    Failed(String),
}

/// A request Jev rejects as too large is halved until it fits; a single
/// hunk that still does not fit gets an error reply.
pub(crate) fn send_all<F>(
    send: &F,
    requests: Vec<JevRequest>,
) -> Vec<(JevRequest, Result<String, String>)>
where
    F: Fn(&JevRequest) -> Result<String, SendError> + Sync,
{
    std::thread::scope(|scope| {
        let workers: Vec<_> = requests
            .into_iter()
            .map(|request| scope.spawn(move || send_fitting(send, request)))
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    })
}

fn send_fitting<F>(send: &F, request: JevRequest) -> Vec<(JevRequest, Result<String, String>)>
where
    F: Fn(&JevRequest) -> Result<String, SendError> + Sync,
{
    match send(&request) {
        Ok(body) => vec![(request, Ok(body))],
        Err(SendError::Failed(message)) => vec![(request, Err(message))],
        Err(SendError::TooLarge) => match request.parts.halves() {
            Some((first, second)) => {
                let mut replies = send_fitting(send, first);
                replies.extend(send_fitting(send, second));
                replies
            }
            None => vec![(request, Err("hunk too large for Jev".to_string()))],
        },
    }
}

/// A file shows its judged hunk with the highest attention, the first
/// one on a tie.
pub(crate) fn file_notes(changes: &ChangeSet, known: &Judgments) -> Vec<FileNote> {
    changes
        .files
        .iter()
        .map(|file| {
            if is_lockfile_path(&file.path) {
                return FileNote {
                    tag: Some("lockfile"),
                    attention: None,
                    low: true,
                    breaking: false,
                };
            }
            if never_sent(&file.path) {
                return FileNote::default();
            }
            let judged: Vec<&Judgment> = file
                .hunks
                .iter()
                .filter_map(|hunk| known.0.get(&HunkKey::of(&file.path, hunk)))
                .collect();
            let mut best: Option<&Judgment> = None;
            for judgment in &judged {
                if best.is_none_or(|b| judgment.attention > b.attention) {
                    best = Some(judgment);
                }
            }
            let mut note = best.map_or_else(FileNote::default, Judgment::note);
            note.breaking = judged.iter().any(|j| j.breaking >= BREAKING_THRESHOLD);
            note
        })
        .collect()
}

#[cfg(test)]
mod tests;
