//! `edit` tool execution: validate an [`EditRequest`], apply its exact
//! text replacements, write the file, and record the trace entry.

use std::fs;
use std::path::Path;

use crate::trace_store::{EditFailureHistoryEntry, EditHistoryEntry, TraceStore};
use crate::{
    EditRequest, SuccessResponse, TextEdit, ToolError, current_timestamp,
    current_working_directory, success_message, tool_error, validate_target_path,
};

pub fn execute_request(request: EditRequest) -> Result<SuccessResponse, ToolError> {
    let store = TraceStore::from_env().map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;
    execute_request_with_trace(&store, request, None)
}

pub fn execute_request_with_trace(
    store: &TraceStore,
    request: EditRequest,
    trace_id: Option<&str>,
) -> Result<SuccessResponse, ToolError> {
    let resolved_trace = store.resolve(trace_id).map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;

    match try_execute_edit(
        store,
        &request,
        &resolved_trace.trace_id,
        resolved_trace.reused,
    ) {
        Ok(response) => Ok(response),
        Err(error) => Err(log_edit_failure(
            store,
            request,
            resolved_trace.trace_id,
            resolved_trace.reused,
            error,
        )),
    }
}

fn try_execute_edit(
    store: &TraceStore,
    request: &EditRequest,
    trace_id: &str,
    reused_trace: bool,
) -> Result<SuccessResponse, String> {
    validate_request(request)?;

    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;

    let original = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    let updated = apply_edit(
        &original,
        &request.old_text,
        &request.new_text,
        &request.path,
    )?;
    let computed = deltoids::Diff::compute(&original, &updated, &request.path);
    let hunks = computed.hunks().to_vec();
    let diff = computed.text().to_string();
    let language = computed.language();
    let highlight = computed.highlight().map(str::to_string);

    fs::write(path, &updated)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    store.append(
        trace_id,
        &EditHistoryEntry {
            v: 2,
            tool: "edit",
            trace_id: trace_id.to_string(),
            timestamp: current_timestamp(),
            cwd: current_working_directory()?,
            path: request.path.clone(),
            reason: request.reason.clone(),
            ok: true,
            edits: vec![trace_edit(request)],
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

fn log_edit_failure(
    store: &TraceStore,
    request: EditRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    let logging_error = store
        .append(
            &trace_id,
            &EditFailureHistoryEntry {
                v: 1,
                tool: "edit",
                trace_id: trace_id.clone(),
                timestamp: current_timestamp(),
                cwd: current_working_directory().unwrap_or_else(|_| String::new()),
                path: request.path.clone(),
                reason: request.reason.clone(),
                ok: false,
                edits: vec![trace_edit(&request)],
                error: error.clone(),
            },
        )
        .err();

    tool_error(trace_id, reused_trace, error, logging_error)
}

fn validate_request(request: &EditRequest) -> Result<(), String> {
    if request.reason.trim().is_empty() {
        return Err("reason must not be empty".to_string());
    }

    if request.old_text.is_empty() {
        return Err("oldText must not be empty".to_string());
    }

    Ok(())
}

/// The one-element trace representation of a flat edit request. The
/// per-edit `reason` mirrors the top-level `reason` so old multi-edit
/// trace renderers keep working.
fn trace_edit(request: &EditRequest) -> TextEdit {
    TextEdit {
        reason: request.reason.clone(),
        old_text: request.old_text.clone(),
        new_text: request.new_text.clone(),
    }
}

pub fn render_diff(original: &str, updated: &str, path: &str) -> String {
    deltoids::Diff::compute(original, updated, path)
        .text()
        .to_string()
}

pub fn apply_edit(
    original: &str,
    old_text: &str,
    new_text: &str,
    path: &str,
) -> Result<String, String> {
    let occurrences = original.match_indices(old_text).collect::<Vec<_>>();
    let (start, matched) = match occurrences.len() {
        0 => {
            return Err(format!(
                "Could not find oldText in {path}. The oldText must match exactly."
            ));
        }
        1 => occurrences[0],
        count => {
            return Err(format!(
                "Found {count} occurrences of oldText in {path}. The oldText must be unique."
            ));
        }
    };

    let mut result = original.to_string();
    result.replace_range(start..start + matched.len(), new_text);

    if result == original {
        return Err(format!(
            "No changes made to {path}. The replacement produced identical content."
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit_request(reason: &str, old_text: &str, new_text: &str) -> EditRequest {
        EditRequest {
            reason: reason.to_string(),
            path: "test.txt".to_string(),
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        }
    }

    #[test]
    fn applies_single_exact_edit() {
        let result = apply_edit("Hello, world!", "world", "pi", "test.txt").unwrap();

        assert_eq!(result, "Hello, pi!");
    }

    #[test]
    fn rejects_missing_text() {
        let error = apply_edit("hello\n", "missing", "x", "test.txt").unwrap_err();

        assert!(error.contains("Could not find"));
    }

    #[test]
    fn rejects_duplicate_text() {
        let error = apply_edit("foo foo foo", "foo", "bar", "test.txt").unwrap_err();

        assert!(error.contains("Found 3 occurrences"));
    }

    #[test]
    fn rejects_no_op_replacement() {
        let error = apply_edit("same", "same", "same", "test.txt").unwrap_err();

        assert!(error.contains("No changes made"));
    }

    #[test]
    fn rejects_empty_old_text() {
        let error = validate_request(&edit_request("Test edit", "", "replacement")).unwrap_err();
        assert!(error.contains("oldText must not be empty"));
    }

    #[test]
    fn rejects_empty_reason() {
        let error = validate_request(&edit_request("", "before", "after")).unwrap_err();
        assert!(error.contains("reason must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_reason() {
        let error = validate_request(&edit_request(" \n\t ", "before", "after")).unwrap_err();
        assert!(error.contains("reason must not be empty"));
    }

    #[test]
    fn renders_a_line_based_diff() {
        let diff = render_diff("const x = 1;\n", "const x = 2;\n", "test.txt");

        assert!(diff.contains("--- original"));
        assert!(diff.contains("+++ modified"));
        assert!(diff.contains("-const x = 1;"));
        assert!(diff.contains("+const x = 2;"));
    }

    #[test]
    fn renders_multiple_changes_in_one_diff() {
        let diff = render_diff(
            "alpha\nbeta\ngamma\ndelta\n",
            "ALPHA\nbeta\nGAMMA\ndelta\n",
            "test.txt",
        );

        assert!(diff.contains("-alpha"));
        assert!(diff.contains("+ALPHA"));
        assert!(diff.contains("-gamma"));
        assert!(diff.contains("+GAMMA"));
    }
}
