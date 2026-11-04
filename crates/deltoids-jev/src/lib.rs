//! Review guidance from TypeSafe's Jev model: what role each hunk of a
//! change plays and how much attention it needs, summarized per file.
//!
//! [`Judgments`] remembers every answer, keyed by a hunk's path and
//! changed lines, so a reload asks only about new hunks. Asking goes
//! through [`Questions::ask`], which takes the HTTP POST as a function:
//! this crate owns the endpoint, the request bodies and their size
//! budgets, retries, and what each response means.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use deltoids::LineKind;
use serde_json::{Map, Value};

mod paths;
mod request;
mod transport;

use paths::{is_lockfile_path, never_sent};
use request::pending_requests;
#[cfg(test)]
use request::{STATE_BUDGET_CHARS, TOTAL_BUDGET_CHARS};
pub use transport::{Post, Reply};

/// A changed file as Jev sees it: its path, how it changed, and its hunks.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    pub hunks: Vec<ChangedHunk>,
}

impl ChangedFile {
    pub fn from_diff(path: &str, status: ChangeStatus, diff: &deltoids::Diff) -> Self {
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
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// A hunk as Jev sees it: the names of the scopes it changes, outermost
/// first, and its removed and added lines without their line endings.
/// Its path and changed lines identify its judgment; the scope does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedHunk {
    pub scope: Vec<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

impl ChangedHunk {
    pub fn from_hunk(hunk: &deltoids::Hunk) -> Self {
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
enum Role {
    Core,
    Refactor,
    Config,
    Deps,
    Build,
    Ci,
    Removal,
    Test,
    Docs,
    Agents,
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
            Self::Agents => "agents",
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
            "agents" => Self::Agents,
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

/// A hunk is critical when Jev puts this much probability on the critical
/// level. The score averages the levels, so a hunk rated 48% critical and
/// 51% careful scores 1.47. On the recorded fixtures, the three hunks Jev
/// leaned critical on scored 0.48 to 0.54 here and every other hunk 0.04
/// or less.
const CRITICAL_PROBABILITY: f64 = 0.3;

/// `attention` runs from 0 (straightforward) to 2 (critical); `critical` is
/// the probability Jev gives the critical level; `breaking` is the
/// probability that the hunk breaks code or users outside its package.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Judgment {
    role: Role,
    attention: f64,
    critical: f64,
    breaking: f64,
}

impl Judgment {
    fn note(&self) -> Note {
        let tag = self.role.name();
        Note {
            tag,
            attention: Some(self.level()),
            low: matches!(tag, "test" | "comments" | "imports"),
        }
    }

    /// A hunk that breaks callers is critical whatever its score.
    fn level(&self) -> Attention {
        if self.breaking >= BREAKING_THRESHOLD || self.critical >= CRITICAL_PROBABILITY {
            Attention::Critical
        } else if self.attention >= 0.5 {
            Attention::Careful
        } else {
            Attention::Straightforward
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attention {
    Straightforward,
    Careful,
    Critical,
}

/// What a hunk or a file shows: the kind of change, as `core` or
/// `lockfile`, how much attention it needs, and whether it is of low value
/// to a reviewer (tests, comment-only and import-only changes, lockfiles).
/// A lockfile is tagged without asking Jev and has no attention level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub tag: &'static str,
    pub attention: Option<Attention>,
    pub low: bool,
}

/// Every judgment Jev has given, by hunk.
#[derive(Default)]
pub struct Judgments(HashMap<HunkKey, Judgment>);

impl Judgments {
    /// What Jev said about `hunk` of the file at `path`, once it has.
    pub fn note(&self, path: &str, hunk: &ChangedHunk) -> Option<Note> {
        self.0.get(&HunkKey::of(path, hunk)).map(Judgment::note)
    }

    /// One note per file, `None` until Jev judged any of its hunks. A file
    /// shows its judged hunk with the highest attention level, then score,
    /// the first one on a tie.
    pub fn file_notes(&self, files: &[ChangedFile]) -> Vec<Option<Note>> {
        file_notes(files, self)
    }

    /// The questions about the hunks of `files` Jev has not answered yet,
    /// `None` when there are none. Lockfiles and generated files are never
    /// asked about.
    pub fn pending(&self, files: &[ChangedFile]) -> Option<Questions> {
        let requests = pending_requests(files, self);
        (!requests.is_empty()).then_some(Questions(requests))
    }

    /// Record every valid answer. On failure, the first error says why,
    /// worded for people; the hunks it left out stay pending.
    pub fn add(&mut self, answers: Answers) -> Result<(), String> {
        let mut first_error = None;
        for (request, reply) in answers.0 {
            let absorbed = reply
                .and_then(|body| absorb(self, &request, &body).map_err(|error| error.to_string()));
            if let Err(error) = absorbed {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

/// Questions about hunks Jev has not judged, packed into requests that fit
/// its token limits.
pub struct Questions(Vec<JevRequest>);

impl Questions {
    /// Ask Jev with `key`, sending each request through `post`, all at once
    /// on threads of their own. Blocks until every request is answered or
    /// has failed. Retries while TypeSafe is busy, and halves a request Jev
    /// rejects as too large.
    pub fn ask(
        self,
        key: &str,
        post: &(dyn Fn(Post<'_>) -> Result<Reply, String> + Sync),
    ) -> Answers {
        let send = |request: &JevRequest| transport::send(key, request, post);
        Answers(send_all(&send, self.0))
    }
}

/// Jev's replies to [`Questions`], for [`Judgments::add`].
pub struct Answers(Vec<(JevRequest, Result<String, String>)>);

#[derive(Debug, PartialEq, Eq)]
enum AbsorbError {
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
fn absorb(known: &mut Judgments, request: &JevRequest, response: &str) -> Result<(), AbsorbError> {
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
        attention: attention.get("score")?.as_f64()?,
        critical: attention.get("probabilities")?.get("2")?.as_f64()?,
        breaking: breaking.get("noul")?.as_f64()?,
    })
}

/// Path plus changed lines, so it survives line shifts and reloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HunkKey(u64);

impl HunkKey {
    fn of(path: &str, hunk: &ChangedHunk) -> Self {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        hunk.removed.hash(&mut hasher);
        hunk.added.hash(&mut hasher);
        Self(hasher.finish())
    }
}

struct JevRequest {
    body: String,
    hunks: Vec<HunkKey>,
    parts: request::Parts,
}

#[derive(Debug, PartialEq, Eq)]
enum SendError {
    TooLarge,
    Failed(String),
}

/// A request Jev rejects as too large is halved until it fits; a single
/// hunk that still does not fit gets an error reply.
fn send_all<F>(send: &F, requests: Vec<JevRequest>) -> Vec<(JevRequest, Result<String, String>)>
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

fn file_notes(files: &[ChangedFile], known: &Judgments) -> Vec<Option<Note>> {
    files
        .iter()
        .map(|file| {
            if is_lockfile_path(&file.path) {
                return Some(Note {
                    tag: "lockfile",
                    attention: None,
                    low: true,
                });
            }
            if never_sent(&file.path) {
                return None;
            }
            let mut best: Option<&Judgment> = None;
            for judgment in file
                .hunks
                .iter()
                .filter_map(|hunk| known.0.get(&HunkKey::of(&file.path, hunk)))
            {
                let rank = |j: &Judgment| (j.level(), j.attention);
                if best.is_none_or(|b| rank(judgment) > rank(b)) {
                    best = Some(judgment);
                }
            }
            best.map(Judgment::note)
        })
        .collect()
}

#[cfg(test)]
mod tests;
