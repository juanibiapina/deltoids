//! Storage for edit/write trace logs.
//!
//! A "trace" is an append-only jsonl log under
//! `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`. This module owns
//! the directory layout, ULID-based trace ids, and (later) the read/write
//! primitives consumed by `execute_*_with_trace` and the TUI.

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
/// store. `reused` is true when the caller supplied an id that already
/// existed; false when a fresh ULID was minted.
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
                last_summary: last_entry.summary.clone(),
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
    /// `Some(id)`: validate as ULID, confirm it exists in this store.
    /// `None`: mint a fresh ULID for a new trace.
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

/// True when `trace_id` parses as a ULID.
pub(crate) fn validate_trace_id(trace_id: &str) -> Result<(), String> {
    Ulid::from_string(trace_id)
        .map(|_| ())
        .map_err(|_| format!("Invalid trace id: {trace_id}"))
}

/// One entry in a trace's `entries.jsonl`.
///
/// Carries the union of fields written by the four private write-side
/// entry structs in `lib.rs`. Optional fields use `#[serde(default)]` so
/// historical entries continue to deserialise.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntry {
    pub v: u8,
    pub tool: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub summary: String,
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
    pub last_summary: String,
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
    fn resolve_rejects_non_ulid_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());

        let err = store.resolve(Some("not-a-ulid")).unwrap_err();

        assert!(err.contains("Invalid trace id"));
    }
}
