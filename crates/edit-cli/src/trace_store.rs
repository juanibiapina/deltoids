//! Storage for edit/write trace logs.
//!
//! A "trace" is an append-only jsonl log under
//! `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`. This module owns
//! the directory layout, ULID-based trace ids, and (later) the read/write
//! primitives consumed by `execute_*_with_trace` and the TUI.

use std::env;
use std::path::PathBuf;

use ulid::Ulid;

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
