//! Crate root for `deltoids-cli`. The edit/write/hashedit/hashread tool
//! execution lives in concern-specific sibling modules; this root keeps
//! the public surface (re-exports), the wire types, and the small shared
//! helpers those modules lean on (timestamps, the working directory,
//! trace success/error shaping, and path validation).

pub mod cli;
pub mod events;
pub mod hashline;
pub mod scroll;
pub mod sidebar;
pub mod sidebar_width;
pub mod terminal;
pub mod trace_store;

mod edit;
mod hash_edit;
mod hash_read;
mod types;
mod write;

use std::env;
use std::path::Path;

use chrono::{SecondsFormat, Utc};

pub use edit::{apply_edit, execute_request, execute_request_with_trace, render_diff};
pub use hash_edit::{execute_hash_edit_request, execute_hash_edit_request_with_trace};
pub use hash_read::execute_hash_read;
pub use trace_store::{
    HistoryEntry, ProjectSummary, TraceStore, TraceSummary, project_id, trace_root_directory,
};
pub use types::{
    EditRequest, ErrorResponse, HashEditOp, HashEditRequest, HashReadRequest, SuccessResponse,
    TextEdit, ToolError, WriteRequest,
};
pub use write::{
    execute_write_request, execute_write_request_with_trace, validate_write_target_path,
};


/// Build the success message shared by every tool: distinguishes a fresh
/// trace from one that was reused so the agent knows to keep the id.
pub(crate) fn success_message(trace_id: &str, reused_trace: bool) -> String {
    if reused_trace {
        format!("Appended to trace {trace_id}.")
    } else {
        format!(
            "Started trace {trace_id}. Reuse this trace id for later edits in the same session."
        )
    }
}

/// Shape a [`ToolError`], folding in any failure that occurred while
/// recording the trace entry itself.
pub(crate) fn tool_error(
    trace_id: String,
    reused_trace: bool,
    error: String,
    logging_error: Option<String>,
) -> ToolError {
    ToolError {
        error: match logging_error {
            Some(logging_error) => {
                format!("{error} Failed to record trace {trace_id}: {logging_error}")
            }
            None => error,
        },
        trace_id: trace_id.clone(),
        message: success_message(&trace_id, reused_trace),
    }
}

pub(crate) fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn current_working_directory() -> Result<String, String> {
    env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|err| format!("Failed to read current directory: {err}"))
}

pub fn read_history_entries(trace_id: &str) -> Result<Vec<HistoryEntry>, String> {
    TraceStore::from_env()?.read(trace_id)
}

pub fn list_traces_for_current_directory() -> Result<Vec<TraceSummary>, String> {
    let cwd = current_working_directory()?;
    TraceStore::from_env()?.list_for_cwd(&cwd)
}

pub fn validate_target_path(path: &Path, display_path: &str) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("Path does not exist: {display_path}"));
    }

    if !path.is_file() {
        return Err(format!("Path is not a file: {display_path}"));
    }

    Ok(())
}
