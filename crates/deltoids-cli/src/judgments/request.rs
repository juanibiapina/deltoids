//! Jev request bodies: the change as state, plus a role and an
//! attention question per hunk, split into requests that fit Jev's
//! token limits.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use super::paths::never_sent;
use super::{ChangeSet, ChangeStatus, ChangedFile, ChangedHunk, HunkKey, JevRequest, Judgments};

const MODEL: &str = "jev-latest";
const MAX_CHANGED_LINES: usize = 40;
const MAX_LINE_CHARS: usize = 200;

/// Jev's limits: 32k tokens for the state plus the longest question,
/// 64k for the whole request. TypeSafe offers no token counter, so sizes
/// are measured in characters of the JSON sent and converted at 2.5
/// characters per token. Real requests measured 3.15 characters per
/// token for diff-heavy state and 3.6 for question text, so 2.5 leaves
/// at least 20% headroom; the budgets keep another 10% back. A request
/// Jev still rejects is halved by [`super::send_all`].
pub(crate) const STATE_BUDGET_CHARS: usize = 72_000;
pub(crate) const TOTAL_BUDGET_CHARS: usize = 144_000;

/// Past this, each request lists only its own files and counts the rest.
const OVERVIEW_BUDGET_CHARS: usize = 24_000;

const ROLE_CRITERIA: [(&str, &str); 12] = [
    (
        "core",
        "implements or changes the behavior this change is about, including updates to the code that calls it",
    ),
    (
        "refactor",
        "restructures existing code without meaning to change behavior, such as extracting, inlining, moving, or renaming across call sites",
    ),
    (
        "removal",
        "deletes code, files, or features without adding a replacement",
    ),
    (
        "config",
        "changes runtime configuration the program reads while it runs: config files, feature flags, environment variables, or default settings; not version numbers",
    ),
    (
        "deps",
        "adds, removes, or upgrades a third-party dependency, in a dependency manifest or vendored code",
    ),
    (
        "build",
        "build scripts and packaging, including release version bumps",
    ),
    (
        "ci",
        "continuous integration workflows and pipeline definitions",
    ),
    (
        "docs",
        "documentation people read to use or work on the project: README, guides, changelogs, release notes, or user-facing help text",
    ),
    (
        "agents",
        "instructions, skills, rules, or prompts written for AI coding agents or language models: AGENTS.md, CLAUDE.md, GEMINI.md, SKILL.md and skill folders, rules and prompt files under .agents, .claude, .cursor, or .github, *.prompt.md files, or prompt text inside source code",
    ),
    (
        "test",
        "tests or test data: test code, fixtures, recorded responses, snapshots, or sample inputs and expected outputs that tests read",
    ),
    (
        "comments",
        "changes only code comments or doc comments inside source files; no code line changes",
    ),
    (
        "imports",
        "changes only import, use, include, or require statements; no other code line changes",
    ),
];

/// The low level names simple behavior changes outright: without that,
/// Jev rated most ordinary code "careful" on a three-level scale.
const ATTENTION_LEVELS: [&str; 3] = [
    "Straightforward: a quick read confirms it, such as simple behavior changes, wiring, renames, or data with nothing subtle",
    "Careful: subtle logic, edge cases, error handling, or a contract between parts of the code that a quick read could get wrong",
    "Critical: touches security, data loss, concurrency, money, migrations, or a public interface",
];

pub(crate) fn pending_requests(changes: &ChangeSet, known: &Judgments) -> Vec<JevRequest> {
    let mut entries = Vec::new();
    for file in &changes.files {
        if never_sent(&file.path) {
            continue;
        }
        for hunk in &file.hunks {
            let key = HunkKey::of(&file.path, hunk);
            if !known.0.contains_key(&key) {
                entries.push(Entry {
                    key,
                    path: file.path.clone(),
                    state: hunk_state(file, hunk),
                });
            }
        }
    }
    let all: Vec<FileLine> = changes
        .files
        .iter()
        .map(|file| FileLine {
            path: file.path.clone(),
            status: status_name(file.status),
        })
        .collect();
    let overview = if file_list_chars(&all) <= OVERVIEW_BUDGET_CHARS {
        Overview::All(Arc::new(all))
    } else {
        Overview::Own {
            total: changes.files.len(),
            statuses: Arc::new(
                all.into_iter()
                    .map(|line| (line.path, line.status))
                    .collect(),
            ),
        }
    };
    pack(entries, &overview)
        .into_iter()
        .map(|batch| {
            Parts {
                overview: overview.clone(),
                entries: batch,
            }
            .into_request()
        })
        .collect()
}

/// What a request was built from, kept so it can be halved.
#[derive(Clone)]
/// Kept so a rejected request can be halved.
pub(crate) struct Parts {
    overview: Overview,
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    key: HunkKey,
    path: String,
    state: Value,
}

#[derive(Clone)]
struct FileLine {
    path: String,
    status: &'static str,
}

#[derive(Clone)]
enum Overview {
    All(Arc<Vec<FileLine>>),
    Own {
        total: usize,
        statuses: Arc<BTreeMap<String, &'static str>>,
    },
}

impl Parts {
    pub(crate) fn halves(&self) -> Option<(JevRequest, JevRequest)> {
        if self.entries.len() < 2 {
            return None;
        }
        let (first, second) = self.entries.split_at(self.entries.len() / 2);
        let half = |entries: &[Entry]| {
            Parts {
                overview: self.overview.clone(),
                entries: entries.to_vec(),
            }
            .into_request()
        };
        Some((half(first), half(second)))
    }

    fn into_request(self) -> JevRequest {
        let mut hunks = Map::new();
        let mut questions = Map::new();
        let mut ids: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (n, entry) in self.entries.iter().enumerate() {
            let id = format!("h{n}");
            ids.entry(&entry.path).or_default().push(id.clone());
            hunks.insert(id.clone(), entry.state.clone());
            questions.insert(format!("role_{id}"), role_question(&id, &entry.path));
            questions.insert(
                format!("attention_{id}"),
                attention_question(&id, &entry.path),
            );
            questions.insert(
                format!("breaking_{id}"),
                breaking_question(&id, &entry.path),
            );
        }
        let mut state = Map::new();
        match &self.overview {
            Overview::All(files) => {
                let lines: Vec<Value> = files
                    .iter()
                    .map(|file| file_line(&file.path, file.status, ids.get(file.path.as_str())))
                    .collect();
                state.insert("files".into(), json!(lines));
            }
            Overview::Own { total, statuses } => {
                let lines: Vec<Value> = ids
                    .iter()
                    .map(|(path, ids)| file_line(path, statuses[*path], Some(ids)))
                    .collect();
                state.insert("other_files".into(), json!(total - lines.len()));
                state.insert("files".into(), json!(lines));
            }
        }
        state.insert("hunks".into(), Value::Object(hunks));
        let body = json!({ "model": MODEL, "state": state, "questions": questions });
        JevRequest {
            body: body.to_string(),
            hunks: self.entries.iter().map(|entry| entry.key).collect(),
            parts: self,
        }
    }
}

fn file_line(path: &str, status: &str, ids: Option<&Vec<String>>) -> Value {
    json!({ "path": path, "status": status, "hunks": ids.cloned().unwrap_or_default() })
}

fn file_list_chars(files: &[FileLine]) -> usize {
    files
        .iter()
        .map(|file| file_line(&file.path, file.status, None).to_string().len() + 1)
        .sum()
}

/// A lone entry over budget still gets its own batch.
fn pack(entries: Vec<Entry>, overview: &Overview) -> Vec<Vec<Entry>> {
    let base = 200
        + match overview {
            Overview::All(files) => file_list_chars(files),
            Overview::Own { .. } => 0,
        };
    let mut batches: Vec<Vec<Entry>> = Vec::new();
    let mut batch = Batch::new(base);
    for entry in entries {
        let cost = Cost::of(&entry, overview, &batch);
        if !batch.entries.is_empty() && !batch.fits(&cost) {
            batches.push(std::mem::take(&mut batch.entries));
            batch = Batch::new(base);
        }
        let cost = Cost::of(&entry, overview, &batch);
        batch.add(entry, &cost);
    }
    if !batch.entries.is_empty() {
        batches.push(batch.entries);
    }
    batches
}

struct Batch {
    entries: Vec<Entry>,
    listed: std::collections::HashSet<String>,
    state: usize,
    total: usize,
    longest: usize,
}

impl Batch {
    fn new(base: usize) -> Self {
        Self {
            entries: Vec::new(),
            listed: Default::default(),
            state: base,
            total: base,
            longest: 0,
        }
    }

    fn fits(&self, cost: &Cost) -> bool {
        self.state + cost.state + self.longest.max(cost.role) <= STATE_BUDGET_CHARS
            && self.total + cost.state + cost.questions() <= TOTAL_BUDGET_CHARS
    }

    fn add(&mut self, entry: Entry, cost: &Cost) {
        self.state += cost.state;
        self.total += cost.state + cost.questions();
        self.longest = self.longest.max(cost.role);
        self.listed.insert(entry.path.clone());
        self.entries.push(entry);
    }
}

struct Cost {
    state: usize,
    role: usize,
    attention: usize,
    breaking: usize,
}

impl Cost {
    fn questions(&self) -> usize {
        self.role + self.attention + self.breaking
    }

    fn of(entry: &Entry, overview: &Overview, batch: &Batch) -> Self {
        const ID_ROOM: usize = 16;
        let id = "h000000";
        let file_line_chars = match overview {
            Overview::Own { statuses, .. } if !batch.listed.contains(&entry.path) => {
                file_line(&entry.path, statuses[&entry.path], None)
                    .to_string()
                    .len()
                    + 1
            }
            _ => 0,
        };
        Self {
            state: entry.state.to_string().len() + ID_ROOM * 2 + file_line_chars,
            role: role_question(id, &entry.path).to_string().len() + ID_ROOM,
            attention: attention_question(id, &entry.path).to_string().len() + ID_ROOM,
            breaking: breaking_question(id, &entry.path).to_string().len() + ID_ROOM,
        }
    }
}

fn hunk_state(file: &ChangedFile, hunk: &ChangedHunk) -> Value {
    let mut state = Map::new();
    state.insert("file".into(), json!(file.path));
    if !hunk.scope.is_empty() {
        state.insert("scope".into(), json!(hunk.scope.join(" > ")));
    }
    state.insert("diff".into(), json!(trimmed_diff(hunk)));
    Value::Object(state)
}

fn changed_lines(hunk: &ChangedHunk) -> impl Iterator<Item = String> + '_ {
    hunk.removed
        .iter()
        .map(|line| format!("-{line}"))
        .chain(hunk.added.iter().map(|line| format!("+{line}")))
        .map(|line| line.chars().take(MAX_LINE_CHARS).collect())
}

fn trimmed_diff(hunk: &ChangedHunk) -> String {
    let lines: Vec<String> = changed_lines(hunk).collect();
    let mut kept: Vec<String> = lines.iter().take(MAX_CHANGED_LINES).cloned().collect();
    if lines.len() > MAX_CHANGED_LINES {
        kept.push(format!(
            "... {} more changed lines",
            lines.len() - MAX_CHANGED_LINES
        ));
    }
    kept.join("\n")
}

/// The path goes into the question itself: Jev weighs it more there than
/// in the state.
fn role_question(id: &str, path: &str) -> Value {
    let criteria: Map<String, Value> = ROLE_CRITERIA
        .iter()
        .map(|(role, meaning)| (role.to_string(), json!(meaning)))
        .collect();
    json!({
        "type": "choice",
        "instructions": format!(
            "What role does the change `hunks.{id}` to the file {path} play in this change? Judge it by that file path as well as its diff: a file under a test or fixtures directory is test data even when its content looks like other code or text."
        ),
        "criteria": criteria,
    })
}

/// Without the path and the sentence on recorded data, Jev rated saved
/// diffs and responses under `fixtures/` routine or careful.
fn attention_question(id: &str, path: &str) -> Value {
    json!({
        "type": "score",
        "instructions": format!(
            "How much careful reviewer attention does the change `hunks.{id}` to the file {path} need? A file under a test or fixtures directory that holds recorded data, such as saved diffs, snapshots, or responses, needs only a skim even when its content looks like code; the tests that read it are where the attention goes."
        ),
        "criteria": ATTENTION_LEVELS,
    })
}

/// With the path, Jev stops flagging items public only inside a binary
/// crate and catches removed CLI commands it missed without it.
fn breaking_question(id: &str, path: &str) -> Value {
    json!({
        "type": "noul",
        "instructions": format!(
            "Does the change `hunks.{id}` to the file {path} break code or users outside its own package or program: a removed or changed exported function, type, or trait, HTTP endpoint, CLI command or flag, config key, or file or wire format? Adding something new is not breaking."
        ),
    })
}

fn status_name(status: ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Added => "added",
        ChangeStatus::Modified => "modified",
        ChangeStatus::Deleted => "deleted",
        ChangeStatus::Renamed => "renamed",
    }
}
