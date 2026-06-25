//! `write` tool execution: validate a [`WriteRequest`], create parent
//! directories, write the full file contents, and record the trace
//! entry.

use std::fs;
use std::path::Path;

use crate::trace_store::{TraceStore, WriteFailureHistoryEntry, WriteHistoryEntry};
use crate::{
    SuccessResponse, ToolError, WriteRequest, current_timestamp, current_working_directory,
    success_message, tool_error,
};

pub fn execute_write_request(request: WriteRequest) -> Result<SuccessResponse, ToolError> {
    let store = TraceStore::from_env().map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;
    execute_write_request_with_trace(&store, request, None)
}

pub fn execute_write_request_with_trace(
    store: &TraceStore,
    request: WriteRequest,
    trace_id: Option<&str>,
) -> Result<SuccessResponse, ToolError> {
    let resolved_trace = store.resolve(trace_id).map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;

    match try_execute_write(
        store,
        &request,
        &resolved_trace.trace_id,
        resolved_trace.reused,
    ) {
        Ok(response) => Ok(response),
        Err(error) => Err(log_write_failure(
            store,
            request,
            resolved_trace.trace_id,
            resolved_trace.reused,
            error,
        )),
    }
}

fn try_execute_write(
    store: &TraceStore,
    request: &WriteRequest,
    trace_id: &str,
    reused_trace: bool,
) -> Result<SuccessResponse, String> {
    validate_write_request(request)?;

    let path = Path::new(&request.path);
    validate_write_target_path(path, &request.path)?;

    let original = if path.exists() {
        fs::read_to_string(path)
            .map_err(|err| format!("Failed to read {}: {}", request.path, err))?
    } else {
        String::new()
    };
    let computed = deltoids::Diff::compute(&original, &request.content, &request.path);
    let hunks = computed.hunks().to_vec();
    let diff = computed.text().to_string();
    let language = computed.language();
    let highlight = computed.highlight().map(str::to_string);

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create parent directories for {}: {}",
                request.path, err
            )
        })?;
    }

    fs::write(path, &request.content)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    store.append(
        trace_id,
        &WriteHistoryEntry {
            v: 2,
            tool: "write",
            trace_id: trace_id.to_string(),
            timestamp: current_timestamp(),
            cwd: current_working_directory()?,
            path: request.path.clone(),
            reason: request.reason.clone(),
            ok: true,
            content: request.content.clone(),
            diff: diff.clone(),
            hunks,
            language,
            highlight,
        },
    )?;

    Ok(SuccessResponse {
        ok: true,
        path: request.path.clone(),
        trace_id: trace_id.to_string(),
        message: success_message(trace_id, reused_trace),
        diff,
    })
}

fn log_write_failure(
    store: &TraceStore,
    request: WriteRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    let logging_error = store
        .append(
            &trace_id,
            &WriteFailureHistoryEntry {
                v: 1,
                tool: "write",
                trace_id: trace_id.clone(),
                timestamp: current_timestamp(),
                cwd: current_working_directory().unwrap_or_else(|_| String::new()),
                path: request.path,
                reason: request.reason,
                ok: false,
                content: request.content,
                error: error.clone(),
            },
        )
        .err();

    tool_error(trace_id, reused_trace, error, logging_error)
}

fn validate_write_request(request: &WriteRequest) -> Result<(), String> {
    if request.reason.trim().is_empty() {
        return Err("reason must not be empty".to_string());
    }

    Ok(())
}

pub fn validate_write_target_path(path: &Path, display_path: &str) -> Result<(), String> {
    if path.exists() && !path.is_file() {
        return Err(format!("Path is not a file: {display_path}"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::trace_store::TraceStore;

    #[test]
    fn writes_full_content_and_returns_diff() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{\n  \"version\": 1\n}\n").unwrap();

        let response = execute_write_request_with_trace(
            &store,
            WriteRequest {
                reason: "Rewrite config".to_string(),
                path: path.to_string_lossy().into_owned(),
                content: "{\n  \"version\": 2\n}\n".to_string(),
            },
            None,
        )
        .unwrap();

        assert!(response.ok);
        assert!(response.trace_id.len() >= 10);
        assert!(response.diff.contains("-  \"version\": 1"));
        assert!(response.diff.contains("+  \"version\": 2"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\n  \"version\": 2\n}\n"
        );
    }

    #[test]
    fn rejects_empty_write_reason() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let error = execute_write_request_with_trace(
            &store,
            WriteRequest {
                reason: String::new(),
                path: "test.txt".to_string(),
                content: "hello\n".to_string(),
            },
            None,
        )
        .unwrap_err();

        assert!(error.error.contains("reason must not be empty"));
    }
}
