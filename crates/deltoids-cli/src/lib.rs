pub mod cli;
pub mod hashline;
pub mod sidebar;
pub mod trace_store;
pub mod tui;

use std::env;
use std::fs;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use trace_store::{
    EditFailureHistoryEntry, EditHistoryEntry, TraceStore, WriteFailureHistoryEntry,
    WriteHistoryEntry,
};
pub use trace_store::{HistoryEntry, TraceSummary, trace_root_directory};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub reason: String,
    pub path: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    #[serde(alias = "summary")] // back-compat with v1/v2 trace entries that used `summary`
    pub reason: String,
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest {
    pub reason: String,
    pub path: String,
    pub content: String,
}

/// A single hashline edit operation as it arrives over JSON. Each
/// variant carries its own `reason` so the trace entry can preserve
/// per-op intent.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum HashEditOp {
    /// Replace one anchored line, or the inclusive range `pos..=end` if
    /// `end` is provided, with `lines`.
    Replace {
        reason: String,
        pos: String,
        #[serde(default)]
        end: Option<String>,
        #[serde(default)]
        lines: Vec<String>,
    },
    /// Insert `lines` before the anchored line. `pos` may be `"BOF"`.
    InsertBefore {
        reason: String,
        pos: String,
        lines: Vec<String>,
    },
    /// Insert `lines` after the anchored line. `pos` may be `"EOF"`.
    InsertAfter {
        reason: String,
        pos: String,
        lines: Vec<String>,
    },
    /// Delete one anchored line, or the inclusive range `pos..=end` if
    /// `end` is provided.
    Delete {
        reason: String,
        pos: String,
        #[serde(default)]
        end: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashEditRequest {
    pub reason: String,
    pub path: String,
    pub edits: Vec<HashEditOp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashReadRequest {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub path: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub message: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub error: String,
    pub trace_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchedEdit {
    index: usize,
    start: usize,
    end: usize,
    new_text: String,
}

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

pub fn execute_hash_edit_request(request: HashEditRequest) -> Result<SuccessResponse, ToolError> {
    let store = TraceStore::from_env().map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;
    execute_hash_edit_request_with_trace(&store, request, None)
}

pub fn execute_hash_edit_request_with_trace(
    store: &TraceStore,
    request: HashEditRequest,
    trace_id: Option<&str>,
) -> Result<SuccessResponse, ToolError> {
    let resolved_trace = store.resolve(trace_id).map_err(|error| ToolError {
        error,
        trace_id: String::new(),
        message: String::new(),
    })?;

    match try_execute_hash_edit(
        store,
        &request,
        &resolved_trace.trace_id,
        resolved_trace.reused,
    ) {
        Ok(response) => Ok(response),
        Err(error) => Err(log_hash_edit_failure(
            store,
            request,
            resolved_trace.trace_id,
            resolved_trace.reused,
            error,
        )),
    }
}

/// Read `path` and return the formatted hashline body (each line as
/// `LINEhh|content`, joined with `\n`). `offset` is the 1-indexed first
/// line to return (default `1`). `limit` is the maximum number of lines
/// (default: all remaining).
pub fn execute_hash_read(request: &HashReadRequest) -> Result<String, String> {
    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;
    let content = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    Ok(render_hash_read(&content, request.offset, request.limit))
}

fn render_hash_read(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    // Strip a single trailing newline so we don't render a phantom empty
    // anchored line at the end of every file.
    let body = content.strip_suffix('\n').unwrap_or(content);
    let start = offset.unwrap_or(1).max(1);
    let lines: Vec<&str> = body.split('\n').collect();
    if start > lines.len() {
        return String::new();
    }
    let end = match limit {
        Some(n) => (start + n).min(lines.len() + 1),
        None => lines.len() + 1,
    };
    let mut out = String::new();
    for (i, line) in lines[start - 1..end - 1].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&hashline::format_hash_line(start + i, line));
    }
    out
}

fn try_execute_hash_edit(
    store: &TraceStore,
    request: &HashEditRequest,
    trace_id: &str,
    reused_trace: bool,
) -> Result<SuccessResponse, String> {
    if request.reason.trim().is_empty() {
        return Err("reason must not be empty".to_string());
    }
    if request.edits.is_empty() {
        return Err("edits must contain at least one operation".to_string());
    }

    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;
    let original = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;

    let engine_edits: Vec<hashline::HashEdit> = request
        .edits
        .iter()
        .enumerate()
        .map(|(idx, op)| op_to_engine(idx, op))
        .collect::<Result<_, _>>()?;

    let applied =
        hashline::apply_hash_edits(&original, &engine_edits).map_err(|err| err.display())?;

    let computed = deltoids::Diff::compute(&original, &applied.text, &request.path);
    let hunks = computed.hunks().to_vec();
    let diff = computed.text().to_string();
    let language = computed.language();

    fs::write(path, &applied.text)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    let synthesised_edits = synthesise_text_edits(&original, &request.edits, &engine_edits);
    store.append(
        trace_id,
        &EditHistoryEntry {
            v: 2,
            tool: "hashedit",
            trace_id: trace_id.to_string(),
            timestamp: current_timestamp(),
            cwd: current_working_directory()?,
            path: request.path.clone(),
            reason: request.reason.clone(),
            ok: true,
            edits: synthesised_edits,
            diff: diff.clone(),
            hunks,
            language,
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

fn op_to_engine(index: usize, op: &HashEditOp) -> Result<hashline::HashEdit, String> {
    let parse_anchor = |token: &str| {
        hashline::Anchor::parse(token).map_err(|err| format!("edits[{index}].pos: {err}"))
    };
    let parse_end = |token: &Option<String>| match token {
        None => Ok(None),
        Some(tok) => hashline::Anchor::parse(tok)
            .map(Some)
            .map_err(|err| format!("edits[{index}].end: {err}")),
    };
    let parse_anchor_or_boundary = |token: &str| {
        hashline::AnchorOrBoundary::parse(token).map_err(|err| format!("edits[{index}].pos: {err}"))
    };
    match op {
        HashEditOp::Replace {
            reason,
            pos,
            end,
            lines,
        } => Ok(hashline::HashEdit::Replace {
            reason: reason.clone(),
            pos: parse_anchor(pos)?,
            end: parse_end(end)?,
            lines: lines.clone(),
        }),
        HashEditOp::InsertBefore { reason, pos, lines } => Ok(hashline::HashEdit::Insert {
            reason: reason.clone(),
            side: hashline::InsertSide::Before,
            pos: parse_anchor_or_boundary(pos)?,
            lines: lines.clone(),
        }),
        HashEditOp::InsertAfter { reason, pos, lines } => Ok(hashline::HashEdit::Insert {
            reason: reason.clone(),
            side: hashline::InsertSide::After,
            pos: parse_anchor_or_boundary(pos)?,
            lines: lines.clone(),
        }),
        HashEditOp::Delete { reason, pos, end } => Ok(hashline::HashEdit::Delete {
            reason: reason.clone(),
            pos: parse_anchor(pos)?,
            end: parse_end(end)?,
        }),
    }
}

/// Synthesise one `TextEdit` per hashline op so the existing trace UI
/// (which renders `EditHistoryEntry.edits`) shows per-op reasons with
/// the actual before/after lines. The synthesised oldText/newText are
/// for display only — they are NOT guaranteed to be re-appliable
/// because ranges may overlap or refer to shifted line numbers if you
/// tried to replay.
fn synthesise_text_edits(
    original: &str,
    ops: &[HashEditOp],
    engine_edits: &[hashline::HashEdit],
) -> Vec<TextEdit> {
    let original_lines: Vec<&str> = original
        .strip_suffix('\n')
        .unwrap_or(original)
        .split('\n')
        .collect();
    let line_at = |n: usize| -> String {
        original_lines
            .get(n - 1)
            .map(|s| (*s).to_owned())
            .unwrap_or_default()
    };
    let join = |xs: &[String]| -> String {
        if xs.is_empty() {
            String::new()
        } else {
            xs.join("\n")
        }
    };
    let range_text = |start: usize, end: usize| -> String {
        (start..=end).map(line_at).collect::<Vec<_>>().join("\n")
    };

    ops.iter()
        .zip(engine_edits.iter())
        .map(|(op, engine)| match (op, engine) {
            (
                HashEditOp::Replace { reason, .. },
                hashline::HashEdit::Replace {
                    pos, end, lines, ..
                },
            ) => TextEdit {
                reason: reason.clone(),
                old_text: range_text(pos.line, end.map_or(pos.line, |e| e.line)),
                new_text: join(lines),
            },
            (HashEditOp::Delete { reason, .. }, hashline::HashEdit::Delete { pos, end, .. }) => {
                TextEdit {
                    reason: reason.clone(),
                    old_text: range_text(pos.line, end.map_or(pos.line, |e| e.line)),
                    new_text: String::new(),
                }
            }
            (HashEditOp::InsertBefore { reason, pos, lines }, _) => TextEdit {
                reason: reason.clone(),
                old_text: format!("<insert before {pos}>"),
                new_text: join(lines),
            },
            (HashEditOp::InsertAfter { reason, pos, lines }, _) => TextEdit {
                reason: reason.clone(),
                old_text: format!("<insert after {pos}>"),
                new_text: join(lines),
            },
            // Unreachable: op variants and engine variants are produced
            // together by op_to_engine; the pairing is one-to-one.
            _ => TextEdit {
                reason: op_reason(op).to_string(),
                old_text: String::new(),
                new_text: String::new(),
            },
        })
        .collect()
}

fn op_reason(op: &HashEditOp) -> &str {
    match op {
        HashEditOp::Replace { reason, .. }
        | HashEditOp::InsertBefore { reason, .. }
        | HashEditOp::InsertAfter { reason, .. }
        | HashEditOp::Delete { reason, .. } => reason,
    }
}

fn log_hash_edit_failure(
    store: &TraceStore,
    request: HashEditRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    // Best-effort: surface each op's reason in the failure entry. We
    // don't synthesise old/new text for failures since we may not have a
    // valid hashline parse to anchor against.
    let placeholder_edits = request
        .edits
        .iter()
        .map(|op| TextEdit {
            reason: op_reason(op).to_string(),
            old_text: String::new(),
            new_text: String::new(),
        })
        .collect();

    let logging_error = store
        .append(
            &trace_id,
            &EditFailureHistoryEntry {
                v: 1,
                tool: "hashedit",
                trace_id: trace_id.clone(),
                timestamp: current_timestamp(),
                cwd: current_working_directory().unwrap_or_default(),
                path: request.path,
                reason: request.reason,
                ok: false,
                edits: placeholder_edits,
                error: error.clone(),
            },
        )
        .err();

    tool_error(trace_id, reused_trace, error, logging_error)
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
    let updated = apply_edits(&original, &request.edits, &request.path)?;
    let computed = deltoids::Diff::compute(&original, &updated, &request.path);
    let hunks = computed.hunks().to_vec();
    let diff = computed.text().to_string();
    let language = computed.language();

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
            edits: request.edits.clone(),
            diff: diff.clone(),
            hunks,
            language,
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

fn success_message(trace_id: &str, reused_trace: bool) -> String {
    if reused_trace {
        format!("Appended to trace {trace_id}.")
    } else {
        format!(
            "Started trace {trace_id}. Reuse this trace id for later edits in the same session."
        )
    }
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
                path: request.path,
                reason: request.reason,
                ok: false,
                edits: request.edits,
                error: error.clone(),
            },
        )
        .err();

    tool_error(trace_id, reused_trace, error, logging_error)
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

fn tool_error(
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

fn validate_request(request: &EditRequest) -> Result<(), String> {
    if request.reason.trim().is_empty() {
        return Err("reason must not be empty".to_string());
    }

    if request.edits.is_empty() {
        return Err("edits must contain at least one replacement".to_string());
    }

    for (index, edit) in request.edits.iter().enumerate() {
        if edit.reason.trim().is_empty() {
            return Err(format!("edits[{index}].reason must not be empty"));
        }

        if edit.old_text.is_empty() {
            return Err(format!("edits[{index}].oldText must not be empty"));
        }
    }

    Ok(())
}

fn validate_write_request(request: &WriteRequest) -> Result<(), String> {
    if request.reason.trim().is_empty() {
        return Err("reason must not be empty".to_string());
    }

    Ok(())
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

pub fn validate_write_target_path(path: &Path, display_path: &str) -> Result<(), String> {
    if path.exists() && !path.is_file() {
        return Err(format!("Path is not a file: {display_path}"));
    }

    Ok(())
}

pub fn render_diff(original: &str, updated: &str, path: &str) -> String {
    deltoids::Diff::compute(original, updated, path)
        .text()
        .to_string()
}

pub fn apply_edits(original: &str, edits: &[TextEdit], path: &str) -> Result<String, String> {
    let mut matches = Vec::with_capacity(edits.len());

    for (index, edit) in edits.iter().enumerate() {
        let occurrences = original.match_indices(&edit.old_text).collect::<Vec<_>>();
        match occurrences.len() {
            0 => {
                return Err(format!(
                    "Could not find edits[{index}] in {path}. The oldText must match exactly."
                ));
            }
            1 => {
                let (start, matched) = occurrences[0];
                matches.push(MatchedEdit {
                    index,
                    start,
                    end: start + matched.len(),
                    new_text: edit.new_text.clone(),
                });
            }
            count => {
                return Err(format!(
                    "Found {count} occurrences of edits[{index}] in {path}. Each oldText must be unique."
                ));
            }
        }
    }

    matches.sort_by_key(|m| m.start);
    for pair in matches.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if previous.end > current.start {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {}. Merge them into one edit or target disjoint regions.",
                previous.index, current.index, path
            ));
        }
    }

    let mut result = original.to_string();
    for matched in matches.iter().rev() {
        result.replace_range(matched.start..matched.end, &matched.new_text);
    }

    if result == original {
        return Err(format!(
            "No changes made to {path}. The replacements produced identical content."
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        EditRequest, HashEditOp, HashEditRequest, HashReadRequest, TextEdit, WriteRequest,
        apply_edits, execute_hash_edit_request_with_trace, execute_hash_read,
        execute_write_request_with_trace, hashline, render_diff, render_hash_read,
        trace_store::TraceStore, validate_request,
    };

    fn anchor_token(line: usize, content: &str) -> String {
        format!("{line}{}", hashline::compute_line_hash(line, content))
    }

    #[test]
    fn applies_single_exact_edit() {
        let result = apply_edits(
            "Hello, world!",
            &[TextEdit {
                reason: "Replace world".to_string(),
                old_text: "world".to_string(),
                new_text: "pi".to_string(),
            }],
            "test.txt",
        )
        .unwrap();

        assert_eq!(result, "Hello, pi!");
    }

    #[test]
    fn applies_multiple_disjoint_edits_against_original_content() {
        let result = apply_edits(
            "foo\nbar\nbaz\n",
            &[
                TextEdit {
                    reason: "Expand foo".to_string(),
                    old_text: "foo\n".to_string(),
                    new_text: "foo bar\n".to_string(),
                },
                TextEdit {
                    reason: "Uppercase bar".to_string(),
                    old_text: "bar\n".to_string(),
                    new_text: "BAR\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap();

        assert_eq!(result, "foo bar\nBAR\nbaz\n");
    }

    #[test]
    fn rejects_missing_text() {
        let error = apply_edits(
            "hello\n",
            &[TextEdit {
                reason: "Replace missing".to_string(),
                old_text: "missing".to_string(),
                new_text: "x".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Could not find"));
    }

    #[test]
    fn rejects_duplicate_text() {
        let error = apply_edits(
            "foo foo foo",
            &[TextEdit {
                reason: "Replace foo".to_string(),
                old_text: "foo".to_string(),
                new_text: "bar".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Found 3 occurrences"));
    }

    #[test]
    fn rejects_overlapping_regions() {
        let error = apply_edits(
            "one\ntwo\nthree\n",
            &[
                TextEdit {
                    reason: "Uppercase first block".to_string(),
                    old_text: "one\ntwo\n".to_string(),
                    new_text: "ONE\nTWO\n".to_string(),
                },
                TextEdit {
                    reason: "Uppercase second block".to_string(),
                    old_text: "two\nthree\n".to_string(),
                    new_text: "TWO\nTHREE\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("overlap"));
    }

    #[test]
    fn rejects_no_op_replacement() {
        let error = apply_edits(
            "same",
            &[TextEdit {
                reason: "No-op replace".to_string(),
                old_text: "same".to_string(),
                new_text: "same".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("No changes made"));
    }

    #[test]
    fn rejects_empty_edits_request() {
        let request = EditRequest {
            reason: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: Vec::new(),
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits must contain at least one replacement"));
    }

    #[test]
    fn rejects_empty_edit_reason() {
        let request = EditRequest {
            reason: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                reason: String::new(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].reason must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_edit_reason() {
        let request = EditRequest {
            reason: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                reason: " \n\t ".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].reason must not be empty"));
    }

    #[test]
    fn rejects_empty_old_text() {
        let request = EditRequest {
            reason: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                reason: "Replace text".to_string(),
                old_text: String::new(),
                new_text: "replacement".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].oldText must not be empty"));
    }

    #[test]
    fn rejects_empty_reason() {
        let request = EditRequest {
            reason: String::new(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                reason: "Replace before".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("reason must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_reason() {
        let request = EditRequest {
            reason: " \n\t ".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                reason: "Replace before".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("reason must not be empty"));
    }

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

    #[test]
    fn does_not_partially_apply_when_one_multi_edit_fails() {
        let original = "alpha\nbeta\ngamma\n";
        let error = apply_edits(
            original,
            &[
                TextEdit {
                    reason: "Uppercase alpha".to_string(),
                    old_text: "alpha\n".to_string(),
                    new_text: "ALPHA\n".to_string(),
                },
                TextEdit {
                    reason: "Try missing line".to_string(),
                    old_text: "missing\n".to_string(),
                    new_text: "MISSING\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Could not find"));
        assert_eq!(original, "alpha\nbeta\ngamma\n");
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

    // ----- hashread -----

    #[test]
    fn render_hash_read_prefixes_every_line_with_anchor() {
        let body = render_hash_read("alpha\nbeta\ngamma\n", None, None);
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("1"));
        assert!(lines[0].ends_with("|alpha"));
        assert!(lines[2].ends_with("|gamma"));
    }

    #[test]
    fn render_hash_read_respects_offset_and_limit() {
        let body = render_hash_read("a\nb\nc\nd\ne\n", Some(2), Some(2));
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("2"));
        assert!(lines[0].ends_with("|b"));
        assert!(lines[1].starts_with("3"));
        assert!(lines[1].ends_with("|c"));
    }

    #[test]
    fn execute_hash_read_returns_anchored_file_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        fs::write(&path, "alpha\nbeta\n").unwrap();

        let body = execute_hash_read(&HashReadRequest {
            path: path.to_string_lossy().into_owned(),
            offset: None,
            limit: None,
        })
        .unwrap();

        assert!(body.contains("|alpha"));
        assert!(body.contains("|beta"));
    }

    #[test]
    fn execute_hash_read_errors_when_path_missing() {
        let err = execute_hash_read(&HashReadRequest {
            path: "/no/such/file/xyz".to_string(),
            offset: None,
            limit: None,
        })
        .unwrap_err();
        assert!(err.contains("Path does not exist"));
    }

    // ----- hashedit -----

    #[test]
    fn execute_hash_edit_applies_replace_with_matching_anchor() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.txt");
        fs::write(&path, "const x = 1;\n").unwrap();

        let response = execute_hash_edit_request_with_trace(
            &store,
            HashEditRequest {
                reason: "Bump x".to_string(),
                path: path.to_string_lossy().into_owned(),
                edits: vec![HashEditOp::Replace {
                    reason: "Bump x to 2".to_string(),
                    pos: anchor_token(1, "const x = 1;"),
                    end: None,
                    lines: vec!["const x = 2;".to_string()],
                }],
            },
            None,
        )
        .unwrap();

        assert!(response.ok);
        assert_eq!(fs::read_to_string(&path).unwrap(), "const x = 2;\n");
        assert!(response.diff.contains("-const x = 1;"));
        assert!(response.diff.contains("+const x = 2;"));
    }

    #[test]
    fn execute_hash_edit_rejects_stale_anchor_and_keeps_file() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.txt");
        fs::write(&path, "const x = 1;\n").unwrap();

        let err = execute_hash_edit_request_with_trace(
            &store,
            HashEditRequest {
                reason: "Bump x".to_string(),
                path: path.to_string_lossy().into_owned(),
                edits: vec![HashEditOp::Replace {
                    reason: "Bump x to 2".to_string(),
                    pos: "1zz".to_string(),
                    end: None,
                    lines: vec!["const x = 2;".to_string()],
                }],
            },
            None,
        )
        .unwrap_err();

        assert!(err.error.contains("Edit rejected"));
        assert!(err.error.contains("|const x = 1;"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "const x = 1;\n");
    }

    #[test]
    fn execute_hash_edit_rejects_empty_reason() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.txt");
        fs::write(&path, "x\n").unwrap();

        let err = execute_hash_edit_request_with_trace(
            &store,
            HashEditRequest {
                reason: "  ".to_string(),
                path: path.to_string_lossy().into_owned(),
                edits: vec![HashEditOp::Replace {
                    reason: "r".to_string(),
                    pos: anchor_token(1, "x"),
                    end: None,
                    lines: vec!["y".to_string()],
                }],
            },
            None,
        )
        .unwrap_err();
        assert!(err.error.contains("reason must not be empty"));
    }

    #[test]
    fn execute_hash_edit_records_synthesised_edits_in_trace() {
        let trace_root = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(trace_root.path().to_path_buf());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.txt");
        fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

        let response = execute_hash_edit_request_with_trace(
            &store,
            HashEditRequest {
                reason: "Mass update".to_string(),
                path: path.to_string_lossy().into_owned(),
                edits: vec![
                    HashEditOp::Replace {
                        reason: "Upper beta".to_string(),
                        pos: anchor_token(2, "beta"),
                        end: None,
                        lines: vec!["BETA".to_string()],
                    },
                    HashEditOp::InsertAfter {
                        reason: "Append footer".to_string(),
                        pos: "EOF".to_string(),
                        lines: vec!["# end".to_string()],
                    },
                ],
            },
            None,
        )
        .unwrap();

        let entries = store.read(&response.trace_id).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.tool, "hashedit");
        assert!(entry.ok);
        assert_eq!(entry.edits.len(), 2);
        assert_eq!(entry.edits[0].reason, "Upper beta");
        assert_eq!(entry.edits[0].old_text, "beta");
        assert_eq!(entry.edits[0].new_text, "BETA");
        assert_eq!(entry.edits[1].reason, "Append footer");
        assert!(entry.edits[1].old_text.contains("<insert after EOF>"));
        assert_eq!(entry.edits[1].new_text, "# end");
    }
}
