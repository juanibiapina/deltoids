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
            let matching_entries = entries
                .iter()
                .filter(|entry| entry.cwd == cwd)
                .collect::<Vec<_>>();
            let Some(last_entry) = matching_entries.last() else {
                continue;
            };

            traces.push(TraceSummary {
                trace_id,
                entry_count: matching_entries.len(),
                last_timestamp: last_entry.timestamp.clone(),
                last_tool: last_entry.tool.clone(),
                last_path: last_entry.path.clone(),
                last_reason: last_entry.reason.clone(),
            });
        }

        traces.sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
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
// Write-side entry shapes
// ---------------------------------------------------------------------------
//
// Four shape variants written by `execute_*_with_trace` and the failure
// loggers, kept separate so type-level invariants stay tight at the
// write site (an ok-edit always has `edits` and `hunks`, a failed-edit
// always has `error`, etc.). All four serialise to the same flat JSON
// shape that `HistoryEntry` deserialises.

#[derive(Debug, serde::Serialize)]
pub(crate) struct EditHistoryEntry {
    pub v: u8,
    pub tool: &'static str,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub reason: String,
    pub ok: bool,
    pub edits: Vec<TextEdit>,
    pub diff: String,
    pub hunks: Vec<deltoids::Hunk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<deltoids::Language>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct EditFailureHistoryEntry {
    pub v: u8,
    pub tool: &'static str,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub reason: String,
    pub ok: bool,
    pub edits: Vec<TextEdit>,
    pub error: String,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct WriteHistoryEntry {
    pub v: u8,
    pub tool: &'static str,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub reason: String,
    pub ok: bool,
    pub content: String,
    pub diff: String,
    pub hunks: Vec<deltoids::Hunk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<deltoids::Language>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct WriteFailureHistoryEntry {
    pub v: u8,
    pub tool: &'static str,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub reason: String,
    pub ok: bool,
    pub content: String,
    pub error: String,
}

/// One entry in a trace's `entries.jsonl`.
///
/// Carries the union of fields written by the four write-side entry
/// structs above. Optional fields use `#[serde(default)]` so historical
/// entries continue to deserialise.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntry {
    pub v: u8,
    pub tool: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    #[serde(alias = "summary")]
    pub reason: String,
    pub ok: bool,
    #[serde(default)]
    pub edits: Vec<TextEdit>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub hunks: Vec<deltoids::Hunk>,
    /// Language detected for the diff (`None` for v1/v2 entries written
    /// before language detection landed, or for unsupported files).
    #[serde(default)]
    pub language: Option<deltoids::Language>,
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

        let entry = serde_json::from_str(line).map_err(|err| {
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

/// Aggregate view of one trace, used by the TUI list pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceSummary {
    pub trace_id: String,
    pub entry_count: usize,
    pub last_timestamp: String,
    pub last_tool: String,
    pub last_path: String,
    pub last_reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
