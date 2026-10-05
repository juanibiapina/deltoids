//! Storage for edit/write trace logs.
//!
//! A "trace" is an append-only jsonl log under
//! `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`. This module owns
//! the directory layout, trace id validation, and the read/write
//! primitives consumed by `execute_*_with_trace` and the TUI.
//!
//! New traces minted by the store get fresh ULIDs. Caller-supplied ids
//! (e.g. a Claude Code `session_id`) are accepted as long as they look
//! like a safe directory name (`[A-Za-z0-9_-]{1,128}`). This lets
//! external integrations key traces on their own session identifiers.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::TextEdit;

/// Handle on a trace root. Tests use `with_root(tempdir)` to bypass
/// `XDG_DATA_HOME`; production uses `from_env()`.
#[derive(Debug, Clone)]
pub struct TraceStore {
    root: PathBuf,
}

/// Result of resolving an optional caller-supplied trace id against the
/// store. `reused` is true when the caller supplied an id whose
/// `entries.jsonl` already exists; false when a fresh ULID was minted
/// or a caller-supplied id is being seen for the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTrace {
    pub(crate) trace_id: String,
    pub(crate) reused: bool,
}

impl TraceStore {
    /// Open a store rooted at `root`. The directory does not need to
    /// exist; entries directories are created lazily on append.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Open a store at the env-resolved trace root
    /// (`$XDG_DATA_HOME/edit/traces`, falling back to
    /// `$HOME/.local/share/edit/traces`).
    pub fn from_env() -> Result<Self, String> {
        Ok(Self::with_root(trace_root_directory()?))
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Path to a single trace's directory under this store.
    pub(crate) fn trace_directory(&self, trace_id: &str) -> PathBuf {
        self.root.join(trace_id)
    }

    /// True when this store already has an entries file for `trace_id`.
    pub fn exists(&self, trace_id: &str) -> bool {
        self.trace_directory(trace_id)
            .join("entries.jsonl")
            .exists()
    }

    /// Append a serializable entry to `trace_id`'s `entries.jsonl`.
    /// Creates the trace directory on first append and takes an
    /// exclusive flock for the duration of the write.
    pub fn append<T: Serialize>(&self, trace_id: &str, entry: &T) -> Result<(), String> {
        let trace_dir = self.trace_directory(trace_id);
        fs::create_dir_all(&trace_dir).map_err(|err| {
            format!(
                "Failed to create trace directory {}: {}",
                trace_dir.display(),
                err
            )
        })?;

        let lock_path = trace_dir.join(".lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|err| format!("Failed to open trace lock {}: {}", lock_path.display(), err))?;
        lock_file
            .lock_exclusive()
            .map_err(|err| format!("Failed to lock trace {trace_id}: {err}"))?;

        let result = (|| {
            let entries_path = trace_dir.join("entries.jsonl");
            let mut entries_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&entries_path)
                .map_err(|err| {
                    format!(
                        "Failed to open trace entries {}: {}",
                        entries_path.display(),
                        err
                    )
                })?;
            serde_json::to_writer(&mut entries_file, entry)
                .map_err(|err| format!("Failed to serialize trace entry: {err}"))?;
            writeln!(&mut entries_file).map_err(|err| {
                format!(
                    "Failed to append trace entry {}: {}",
                    entries_path.display(),
                    err
                )
            })
        })();

        let unlock_result = lock_file.unlock();
        result?;
        unlock_result.map_err(|err| format!("Failed to unlock trace {trace_id}: {err}"))?;
        Ok(())
    }

    /// Aggregate every trace under this store that has at least one entry
    /// recorded in `cwd`. Each `TraceSummary` carries the count and the
    /// last entry's metadata, sorted newest-first.
    pub fn list_for_cwd(&self, cwd: &str) -> Result<Vec<TraceSummary>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }

        let mut traces = Vec::new();
        for raw in self.load_all_raw()? {
            let matching = raw
                .entries
                .iter()
                .filter(|entry| entry.cwd == cwd)
                .collect::<Vec<_>>();
            if let Some(summary) = trace_summary_from(&raw.trace_id, &matching) {
                traces.push(summary);
            }
        }

        traces.sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
        Ok(traces)
    }

    /// Every trace under this store, regardless of directory, summarised
    /// from its last entry and sorted newest-first. Used by the `serve`
    /// subcommand, which browses traces across all projects.
    pub fn list_all(&self) -> Result<Vec<TraceSummary>, String> {
        let mut traces = Vec::new();
        for raw in self.load_all_raw()? {
            let refs = raw.entries.iter().collect::<Vec<_>>();
            if let Some(summary) = trace_summary_from(&raw.trace_id, &refs) {
                traces.push(summary);
            }
        }
        traces.sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
        Ok(traces)
    }

    /// Distinct working directories seen across every trace, each with the
    /// trace/entry counts and last-activity timestamp recorded there.
    /// Sorted by most recent activity first.
    pub fn projects(&self) -> Result<Vec<ProjectSummary>, String> {
        use std::collections::HashMap;

        let mut by_cwd: HashMap<String, ProjectSummary> = HashMap::new();
        for raw in self.load_all_raw()? {
            merge_trace_into_projects(&raw, &mut by_cwd);
        }

        let mut projects = by_cwd.into_values().collect::<Vec<_>>();
        projects.sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
        Ok(projects)
    }

    /// Load every valid trace directory with all of its entries. Shared by
    /// the directory-scanning list methods.
    fn load_all_raw(&self) -> Result<Vec<RawTrace>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }

        let mut traces = Vec::new();
        let directories = fs::read_dir(&self.root)
            .map_err(|err| format!("Failed to read {}: {}", self.root.display(), err))?;
        for directory in directories {
            let directory = directory
                .map_err(|err| format!("Failed to read {}: {}", self.root.display(), err))?;
            let trace_dir = directory.path();
            if !trace_dir.is_dir() {
                continue;
            }

            let trace_id = directory.file_name().to_string_lossy().into_owned();
            if validate_trace_id(&trace_id).is_err() {
                continue;
            }

            let entries_path = trace_dir.join("entries.jsonl");
            if !entries_path.exists() {
                continue;
            }

            let entries = read_history_entries_from_path(&entries_path)?;
            traces.push(RawTrace { trace_id, entries });
        }
        Ok(traces)
    }

    /// Read every entry recorded for `trace_id` in this store.
    /// Validates the id, then loads the jsonl file.
    pub fn read(&self, trace_id: &str) -> Result<Vec<HistoryEntry>, String> {
        validate_trace_id(trace_id)?;
        let entries_path = self.trace_directory(trace_id).join("entries.jsonl");
        if !entries_path.exists() {
            return Err(format!("Trace not found: {trace_id}"));
        }
        read_history_entries_from_path(&entries_path)
    }

    /// Resolve a caller-supplied optional trace id.
    ///
    /// `Some(id)`: validate the id, confirm it exists in this store.
    /// `None`: mint a fresh ULID for a new trace.
    ///
    /// Use this for tools that should fail loudly when the caller
    /// passes an id for a trace that has not yet been started
    /// (`deltoids edit`/`deltoids write`).
    pub(crate) fn resolve(&self, trace_id: Option<&str>) -> Result<ResolvedTrace, String> {
        match trace_id {
            Some(trace_id) => {
                validate_trace_id(trace_id)?;
                if !self.exists(trace_id) {
                    return Err(format!("Trace does not exist: {trace_id}"));
                }
                Ok(ResolvedTrace {
                    trace_id: trace_id.to_string(),
                    reused: true,
                })
            }
            None => Ok(ResolvedTrace {
                trace_id: Ulid::new().to_string(),
                reused: false,
            }),
        }
    }

    /// Like [`resolve`], but accepts a caller-supplied id that does not
    /// yet exist. Used by integrations (e.g. the Claude Code hook) that
    /// key traces on an external session identifier and want to create
    /// the trace on first use.
    pub(crate) fn resolve_or_create(
        &self,
        trace_id: Option<&str>,
    ) -> Result<ResolvedTrace, String> {
        match trace_id {
            Some(trace_id) => {
                validate_trace_id(trace_id)?;
                let reused = self.exists(trace_id);
                Ok(ResolvedTrace {
                    trace_id: trace_id.to_string(),
                    reused,
                })
            }
            None => Ok(ResolvedTrace {
                trace_id: Ulid::new().to_string(),
                reused: false,
            }),
        }
    }
}

/// Root directory containing every trace for the current data home.
///
/// Internal callers should prefer `TraceStore::from_env()` which carries
/// this root for subsequent operations. Exposed for the TUI's filesystem
/// watcher, which needs the path itself rather than a store handle.
pub fn trace_root_directory() -> Result<PathBuf, String> {
    Ok(data_home_directory()?.join("edit").join("traces"))
}

/// Resolve the data-home directory, honouring `XDG_DATA_HOME` then `HOME`.
pub(crate) fn data_home_directory() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }

    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".local").join("share"));
    }

    Err("Could not determine data home directory".to_string())
}

/// True when `trace_id` is a safe directory name we are willing to use
/// for a trace folder. Accepts ULIDs minted by the store as well as
/// external session ids like Claude Code's UUID `session_id`.
pub(crate) fn validate_trace_id(trace_id: &str) -> Result<(), String> {
    if trace_id.is_empty() {
        return Err("Invalid trace id: ".to_string());
    }
    if trace_id.len() > 128 {
        return Err(format!("Invalid trace id: {trace_id}"));
    }
    if !trace_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("Invalid trace id: {trace_id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry shape
// ---------------------------------------------------------------------------

/// Version written on new entries. Informational: the reader tells the
/// grouped shape from older flat records by the presence of `files`.
pub(crate) const ENTRY_VERSION: u8 = 4;

/// One entry in a trace's `entries.jsonl`: one tool invocation and the
/// files it changed (zero, one, or several).
///
/// Entries written before multi-file support stored a single file's fields
/// at the top level. Those records load as an entry with one file; the
/// conversion lives in [`WireEntry`] and nothing else knows about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "WireEntry")]
pub struct HistoryEntry {
    pub v: u8,
    pub tool: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub reason: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub files: Vec<FileChange>,
}

/// One file changed (or attempted) by an entry's invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<TextEdit>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<deltoids::Hunk>,
    /// Language detected for the diff (`None` for entries written before
    /// language detection landed, or for unsupported files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<deltoids::Language>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
}

impl HistoryEntry {
    /// An entry for an invocation that touched one file. `ok` follows
    /// from whether an `error` is present.
    pub(crate) fn single_file(
        tool: &str,
        trace_id: &str,
        cwd: String,
        reason: String,
        error: Option<String>,
        file: FileChange,
    ) -> Self {
        Self {
            v: ENTRY_VERSION,
            tool: tool.to_string(),
            trace_id: trace_id.to_string(),
            timestamp: crate::current_timestamp(),
            cwd,
            reason,
            ok: error.is_none(),
            error,
            files: vec![file],
        }
    }

    /// Paths of every file in this entry, in stored order.
    pub fn paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
}

/// The on-disk record: either the grouped shape (`files`) or an older
/// flat record whose single file's fields sit at the top level.
#[derive(Deserialize)]
struct WireEntry {
    v: u8,
    tool: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    timestamp: String,
    cwd: String,
    #[serde(alias = "summary")]
    reason: String,
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    files: Option<Vec<FileChange>>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    edits: Vec<TextEdit>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    diff: Option<String>,
    #[serde(default)]
    hunks: Vec<deltoids::Hunk>,
    #[serde(default)]
    language: Option<deltoids::Language>,
    #[serde(default)]
    highlight: Option<String>,
}

impl TryFrom<WireEntry> for HistoryEntry {
    type Error = String;

    fn try_from(wire: WireEntry) -> Result<Self, Self::Error> {
        let files = match (wire.files, wire.path) {
            (Some(files), _) => files,
            (None, Some(path)) => vec![FileChange {
                path,
                edits: wire.edits,
                content: wire.content,
                diff: wire.diff,
                hunks: wire.hunks,
                language: wire.language,
                highlight: wire.highlight,
            }],
            (None, None) => return Err("history entry has neither files nor path".to_string()),
        };
        Ok(Self {
            v: wire.v,
            tool: wire.tool,
            trace_id: wire.trace_id,
            timestamp: wire.timestamp,
            cwd: wire.cwd,
            reason: wire.reason,
            ok: wire.ok,
            error: wire.error,
            files,
        })
    }
}

/// Parse a trace's `entries.jsonl` file.
fn read_history_entries_from_path(entries_path: &Path) -> Result<Vec<HistoryEntry>, String> {
    let contents = fs::read_to_string(entries_path)
        .map_err(|err| format!("Failed to read {}: {}", entries_path.display(), err))?;
    let mut entries = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let entry = parse_history_entry(line).map_err(|err| {
            format!(
                "Failed to parse history entry {} in {}: {}",
                index + 1,
                entries_path.display(),
                err
            )
        })?;
        entries.push(entry);
    }

    Ok(entries)
}

pub(crate) fn parse_history_entry(line: &str) -> Result<HistoryEntry, serde_json::Error> {
    serde_json::from_str(line)
}

/// Aggregate view of one trace, used by the TUI list pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceSummary {
    pub trace_id: String,
    /// Working directory of the trace's last (matching) entry. Identifies
    /// the project the trace belongs to for the `serve` subcommand.
    pub cwd: String,
    pub entry_count: usize,
    pub last_timestamp: String,
    pub last_tool: String,
    /// Every file of the trace's last (matching) entry.
    pub last_paths: Vec<String>,
    pub last_reason: String,
}

/// A trace directory loaded whole: its id plus every entry, unfiltered.
struct RawTrace {
    trace_id: String,
    entries: Vec<HistoryEntry>,
}

/// Build a [`TraceSummary`] from a trace id and the entries that belong to
/// it (already filtered by the caller). `None` when there are no entries.
pub(crate) fn trace_summary_from(
    trace_id: &str,
    entries: &[&HistoryEntry],
) -> Option<TraceSummary> {
    let last = entries.last()?;
    Some(TraceSummary {
        trace_id: trace_id.to_string(),
        cwd: last.cwd.clone(),
        entry_count: entries.len(),
        last_timestamp: last.timestamp.clone(),
        last_tool: last.tool.clone(),
        last_paths: last.paths(),
        last_reason: last.reason.clone(),
    })
}

/// Fold one trace's entries into the per-directory project aggregates:
/// bump each project's entry count and last-activity timestamp, then count
/// the trace once for every distinct directory it touched.
fn merge_trace_into_projects(
    raw: &RawTrace,
    by_cwd: &mut std::collections::HashMap<String, ProjectSummary>,
) {
    let mut cwds_in_trace = std::collections::HashSet::new();
    for entry in &raw.entries {
        let project = by_cwd
            .entry(entry.cwd.clone())
            .or_insert_with(|| ProjectSummary::empty(&entry.cwd));
        project.entry_count += 1;
        if entry.timestamp > project.last_timestamp {
            project.last_timestamp = entry.timestamp.clone();
        }
        cwds_in_trace.insert(entry.cwd.clone());
    }
    for cwd in cwds_in_trace {
        if let Some(project) = by_cwd.get_mut(&cwd) {
            project.trace_count += 1;
        }
    }
}

/// Aggregate view of one project (a distinct working directory) across all
/// of its traces. Used by the `serve` subcommand's project list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectSummary {
    /// Opaque, stable, URL-safe id derived from `cwd`.
    pub id: String,
    pub cwd: String,
    /// Last path component of `cwd`, for display.
    pub name: String,
    pub trace_count: usize,
    pub entry_count: usize,
    pub last_timestamp: String,
}

impl ProjectSummary {
    fn empty(cwd: &str) -> Self {
        Self {
            id: project_id(cwd),
            cwd: cwd.to_string(),
            name: project_name(cwd),
            trace_count: 0,
            entry_count: 0,
            last_timestamp: String::new(),
        }
    }
}

/// Stable URL-safe id for a project directory (xxh32 of the path, hex).
pub fn project_id(cwd: &str) -> String {
    format!("{:08x}", xxhash_rust::xxh32::xxh32(cwd.as_bytes(), 0))
}

fn project_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| cwd.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUNK: &str = r#"{"old_start":1,"new_start":1,"lines":[{"kind":"Added","content":"let x = 1;"}],"ancestors":[]}"#;

    fn write_log(store: &TraceStore, trace_id: &str, lines: &[String]) {
        let dir = store.trace_directory(trace_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entries.jsonl"), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn old_flat_records_load_as_one_file_and_new_records_keep_every_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        let lines = [
            r#"{"v":1,"tool":"edit","traceId":"t","timestamp":"1","cwd":"/p","path":"/p/a.rs","summary":"old","ok":false,"edits":[{"summary":"old","oldText":"a","newText":"b"}],"error":"no match"}"#.to_string(),
            format!(r#"{{"v":2,"tool":"write","traceId":"t","timestamp":"2","cwd":"/p","path":"/p/b.rs","reason":"rewrite","ok":true,"content":"x","diff":"d","hunks":[{HUNK}]}}"#),
            format!(r#"{{"v":4,"tool":"bash","traceId":"t","timestamp":"3","cwd":"/p","reason":"format","ok":true,"files":[{{"path":"/p/c.rs","hunks":[{HUNK}]}},{{"path":"/p/d.rs"}}]}}"#),
        ];
        write_log(&store, "t", &lines);

        let entries = store.read("t").unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].paths(), ["/p/a.rs"]);
        assert_eq!(entries[0].reason, "old");
        assert_eq!(entries[0].error.as_deref(), Some("no match"));
        assert_eq!(entries[0].files[0].edits[0].old_text, "a");
        assert_eq!(entries[1].paths(), ["/p/b.rs"]);
        assert_eq!(entries[1].files[0].content, "x");
        assert_eq!(entries[1].files[0].hunks.len(), 1);
        assert_eq!(entries[2].paths(), ["/p/c.rs", "/p/d.rs"]);
        assert_eq!(entries[2].files[0].hunks.len(), 1);
    }

    #[test]
    fn single_file_entries_round_trip_through_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        let file = FileChange {
            path: "/p/a.rs".to_string(),
            diff: Some("d".to_string()),
            ..FileChange::default()
        };
        store
            .append(
                "t",
                &HistoryEntry::single_file(
                    "edit",
                    "t",
                    "/p".into(),
                    "why".into(),
                    None,
                    file.clone(),
                ),
            )
            .unwrap();
        store
            .append(
                "t",
                &HistoryEntry::single_file(
                    "edit",
                    "t",
                    "/p".into(),
                    "why".into(),
                    Some("boom".into()),
                    file,
                ),
            )
            .unwrap();

        let entries = store.read("t").unwrap();

        assert!(entries[0].ok && entries[0].error.is_none());
        assert!(!entries[1].ok);
        assert_eq!(entries[1].error.as_deref(), Some("boom"));
        assert_eq!(entries[1].paths(), ["/p/a.rs"]);
        assert_eq!(entries[1].files[0].diff.as_deref(), Some("d"));
    }

    #[test]
    fn a_record_without_files_or_path_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        write_log(
            &store,
            "t",
            &[r#"{"v":4,"tool":"edit","traceId":"t","timestamp":"1","cwd":"/p","reason":"r","ok":true}"#.to_string()],
        );

        assert!(store.read("t").is_err());
    }

    #[test]
    fn a_multi_file_entry_counts_once_and_lists_every_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        write_log(
            &store,
            "t",
            &[r#"{"v":4,"tool":"bash","traceId":"t","timestamp":"1","cwd":"/p","reason":"r","ok":true,"files":[{"path":"/p/a.rs"},{"path":"/p/b.rs"}]}"#.to_string()],
        );

        let summary = &store.list_all().unwrap()[0];

        assert_eq!(summary.entry_count, 1);
        assert_eq!(summary.last_paths, ["/p/a.rs", "/p/b.rs"]);
        assert_eq!(store.projects().unwrap()[0].entry_count, 1);
    }

    #[test]
    fn with_root_mints_fresh_ulid_when_no_id_provided() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        let resolved = store.resolve(None).unwrap();

        assert!(!resolved.reused);
        assert!(Ulid::from_string(&resolved.trace_id).is_ok());
    }

    #[test]
    fn with_root_rejects_unknown_trace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        let unknown = Ulid::new().to_string();

        let err = store.resolve(Some(&unknown)).unwrap_err();

        assert!(err.contains("Trace does not exist"));
        assert!(err.contains(&unknown));
    }

    #[test]
    fn resolve_rejects_unsafe_trace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        let err = store.resolve(Some("bad/trace/id")).unwrap_err();

        assert!(err.contains("Invalid trace id"));
    }

    #[test]
    fn resolve_accepts_a_uuid_style_external_trace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        // Caller-supplied UUID-style id (e.g. a Claude Code session_id).
        // `resolve` still requires it to exist; it should validate as
        // a safe id and only fail with "Trace does not exist".
        let session_id = "40cc627a-e96a-41bb-8259-ae81589f5599";
        let err = store.resolve(Some(session_id)).unwrap_err();

        assert!(err.contains("Trace does not exist"));
    }

    #[test]
    fn resolve_or_create_accepts_a_new_external_trace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        let session_id = "40cc627a-e96a-41bb-8259-ae81589f5599";
        let resolved = store.resolve_or_create(Some(session_id)).unwrap();

        assert_eq!(resolved.trace_id, session_id);
        assert!(!resolved.reused);
    }

    #[test]
    fn resolve_or_create_marks_reused_when_trace_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        let session_id = "40cc627a-e96a-41bb-8259-ae81589f5599";
        // Create the trace by appending a placeholder entry.
        store
            .append(
                session_id,
                &serde_json::json!({"v": 1, "placeholder": true}),
            )
            .unwrap();

        let resolved = store.resolve_or_create(Some(session_id)).unwrap();

        assert_eq!(resolved.trace_id, session_id);
        assert!(resolved.reused);
    }

    fn append_entry(store: &TraceStore, trace_id: &str, cwd: &str, timestamp: &str) {
        store
            .append(
                trace_id,
                &serde_json::json!({
                    "v": 3,
                    "tool": "edit",
                    "traceId": trace_id,
                    "timestamp": timestamp,
                    "cwd": cwd,
                    "path": format!("{cwd}/app.rs"),
                    "reason": "change",
                    "ok": true,
                }),
            )
            .unwrap();
    }

    #[test]
    fn list_all_returns_traces_from_every_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        append_entry(
            &store,
            "01JAAAAAAAAAAAAAAAAAAAAAAA",
            "/a",
            "2026-01-01T00:00:00Z",
        );
        append_entry(
            &store,
            "01JBBBBBBBBBBBBBBBBBBBBBBB",
            "/b",
            "2026-01-02T00:00:00Z",
        );

        let all = store.list_all().unwrap();

        assert_eq!(all.len(), 2);
        // Sorted newest-first.
        assert_eq!(all[0].trace_id, "01JBBBBBBBBBBBBBBBBBBBBBBB");
        assert_eq!(all[0].cwd, "/b");
        assert_eq!(all[1].cwd, "/a");
    }

    #[test]
    fn projects_aggregate_traces_and_entries_by_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        // Two traces in /a (three entries total), one in /b.
        append_entry(
            &store,
            "01JAAAAAAAAAAAAAAAAAAAAAAA",
            "/a",
            "2026-01-01T00:00:00Z",
        );
        append_entry(
            &store,
            "01JAAAAAAAAAAAAAAAAAAAAAAA",
            "/a",
            "2026-01-01T00:01:00Z",
        );
        append_entry(
            &store,
            "01JCCCCCCCCCCCCCCCCCCCCCCC",
            "/a",
            "2026-01-03T00:00:00Z",
        );
        append_entry(
            &store,
            "01JBBBBBBBBBBBBBBBBBBBBBBB",
            "/b",
            "2026-01-02T00:00:00Z",
        );

        let projects = store.projects().unwrap();

        assert_eq!(projects.len(), 2);
        // Sorted newest-first: /a's last activity is 2026-01-03.
        let project_a = &projects[0];
        assert_eq!(project_a.cwd, "/a");
        assert_eq!(project_a.name, "a");
        assert_eq!(project_a.trace_count, 2);
        assert_eq!(project_a.entry_count, 3);
        assert_eq!(project_a.last_timestamp, "2026-01-03T00:00:00Z");
        assert_eq!(project_a.id, project_id("/a"));

        let project_b = &projects[1];
        assert_eq!(project_b.cwd, "/b");
        assert_eq!(project_b.trace_count, 1);
        assert_eq!(project_b.entry_count, 1);
    }

    #[test]
    fn list_all_is_empty_when_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().join("missing"));
        assert!(store.list_all().unwrap().is_empty());
        assert!(store.projects().unwrap().is_empty());
    }
}
