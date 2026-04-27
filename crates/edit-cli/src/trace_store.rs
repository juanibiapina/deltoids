//! Storage for edit/write trace logs.
//!
//! A "trace" is an append-only jsonl log under
//! `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`. This module owns
//! the directory layout, ULID-based trace ids, and (later) the read/write
//! primitives consumed by `execute_*_with_trace` and the TUI.

use std::env;
use std::path::PathBuf;

use serde::Deserialize;
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

/// Path to a single trace's directory under the trace root.
pub(crate) fn trace_directory(trace_id: &str) -> Result<PathBuf, String> {
    Ok(trace_root_directory()?.join(trace_id))
}

/// Root directory containing every trace for the current data home.
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
