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
