//! `hashedit` tool execution: translate JSON [`HashEditOp`]s into engine
//! [`hashline::HashEdit`]s, apply them with anchor validation, write the
//! file, and record a trace entry with synthesised per-op [`TextEdit`]s.

use std::fs;
use std::path::Path;

use crate::trace_store::{EditFailureHistoryEntry, EditHistoryEntry, TraceStore};
use crate::{
    HashEditOp, HashEditRequest, SuccessResponse, TextEdit, ToolError, current_timestamp,
    current_working_directory, hashline, success_message, tool_error, validate_target_path,
};

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
    let highlight = computed.highlight().map(str::to_string);

    fs::write(path, &applied.text)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    let synthesised_edits =
        synthesise_text_edits(&original, &request.reason, &request.edits, &engine_edits);
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
        HashEditOp::Replace { pos, end, lines } => Ok(hashline::HashEdit::Replace {
            pos: parse_anchor(pos)?,
            end: parse_end(end)?,
            lines: lines.clone(),
        }),
        HashEditOp::InsertBefore { pos, lines } => Ok(hashline::HashEdit::Insert {
            side: hashline::InsertSide::Before,
            pos: parse_anchor_or_boundary(pos)?,
            lines: lines.clone(),
        }),
        HashEditOp::InsertAfter { pos, lines } => Ok(hashline::HashEdit::Insert {
            side: hashline::InsertSide::After,
            pos: parse_anchor_or_boundary(pos)?,
            lines: lines.clone(),
        }),
        HashEditOp::Delete { pos, end } => Ok(hashline::HashEdit::Delete {
            pos: parse_anchor(pos)?,
            end: parse_end(end)?,
        }),
    }
}

/// Synthesise one `TextEdit` per hashline op so the existing trace UI
/// (which renders `EditHistoryEntry.edits`) shows the actual before/after
/// lines. Each edit carries the request's top-level `reason` (ops no
/// longer have their own). The synthesised oldText/newText are for
/// display only — they are NOT guaranteed to be re-appliable because
/// ranges may overlap or refer to shifted line numbers if you tried to
/// replay.
fn synthesise_text_edits(
    original: &str,
    reason: &str,
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
                HashEditOp::Replace { .. },
                hashline::HashEdit::Replace {
                    pos, end, lines, ..
                },
            ) => TextEdit {
                reason: reason.to_string(),
                old_text: range_text(pos.line, end.map_or(pos.line, |e| e.line)),
                new_text: join(lines),
            },
            (HashEditOp::Delete { .. }, hashline::HashEdit::Delete { pos, end, .. }) => TextEdit {
                reason: reason.to_string(),
                old_text: range_text(pos.line, end.map_or(pos.line, |e| e.line)),
                new_text: String::new(),
            },
            (HashEditOp::InsertBefore { pos, lines }, _) => TextEdit {
                reason: reason.to_string(),
                old_text: format!("<insert before {pos}>"),
                new_text: join(lines),
            },
            (HashEditOp::InsertAfter { pos, lines }, _) => TextEdit {
                reason: reason.to_string(),
                old_text: format!("<insert after {pos}>"),
                new_text: join(lines),
            },
            // Unreachable: op variants and engine variants are produced
            // together by op_to_engine; the pairing is one-to-one.
            _ => TextEdit {
                reason: reason.to_string(),
                old_text: String::new(),
                new_text: String::new(),
            },
        })
        .collect()
}

fn log_hash_edit_failure(
    store: &TraceStore,
    request: HashEditRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    // Best-effort: one placeholder edit per op carrying the request's
    // top-level reason. We don't synthesise old/new text for failures
    // since we may not have a valid hashline parse to anchor against.
    let placeholder_edits = request
        .edits
        .iter()
        .map(|_op| TextEdit {
            reason: request.reason.clone(),
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::trace_store::TraceStore;

    fn anchor_token(line: usize, content: &str) -> String {
        format!("{line}{}", hashline::compute_line_hash(line, content))
    }

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
                        pos: anchor_token(2, "beta"),
                        end: None,
                        lines: vec!["BETA".to_string()],
                    },
                    HashEditOp::InsertAfter {
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
        // Every synthesised edit carries the request's top-level reason.
        assert_eq!(entry.edits[0].reason, "Mass update");
        assert_eq!(entry.edits[0].old_text, "beta");
        assert_eq!(entry.edits[0].new_text, "BETA");
        assert_eq!(entry.edits[1].reason, "Mass update");
        assert!(entry.edits[1].old_text.contains("<insert after EOF>"));
        assert_eq!(entry.edits[1].new_text, "# end");
    }
}
