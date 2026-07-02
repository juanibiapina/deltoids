//! `deltoids hook` — adapters that turn external coding-agent
//! lifecycle events into trace entries.
//!
//! Today the only adapter is `claude-code`, which consumes the JSON
//! envelope Claude Code pipes to a `PostToolUse` command hook
//! registered for `Write` and `Edit`. Other adapters (codex, …) can
//! land here as siblings.
//!
//! Design contract:
//!
//! * The hook never blocks the agent. It always exits 0 on success and
//!   exits 1 (never 2) on failure, so Claude Code never receives our
//!   stderr as feedback.
//! * Stdout is intentionally empty so the user's transcript view stays
//!   uncluttered.
//! * Unknown tools or shapes are no-ops. We accept liberally because
//!   Claude Code's hook envelope is partially undocumented and may
//!   gain fields over time.
//! * The deltoids trace id is the Claude Code `session_id`. Every tool
//!   call inside one Claude session lands in the same trace.
//!
//! Captured fixtures from a real Claude Code session (Bedrock backend)
//! live under `crates/tests/fixtures/claude-code/`. The tests exercise
//! this subcommand with those payloads verbatim.
//!
//! `tool_response.originalFile` carries the file content as Claude Code
//! observed it before the edit; we trust that as the "before" snapshot
//! and read disk for the "after" so any post-edit mutations (other
//! hooks, formatters) are captured faithfully.

use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::Path;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Subcommand};
use serde::Deserialize;

use crate::trace_store::{
    EditFailureHistoryEntry, TraceStore, WriteFailureHistoryEntry, WriteHistoryEntry,
};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub adapter: Adapter,
}

#[derive(Debug, Subcommand)]
pub enum Adapter {
    /// Claude Code `PostToolUse` hook: record Write and Edit calls.
    ClaudeCode,
}

pub fn run(args: Args) -> ExitCode {
    match args.adapter {
        Adapter::ClaudeCode => run_claude_code(),
    }
}

fn run_claude_code() -> ExitCode {
    match run_claude_code_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Exit 1, not 2. Exit 2 would feed our stderr back to
            // Claude as blocking feedback; we never want that. Exit 1
            // is non-blocking: the user sees the error in the
            // transcript, the agent does not.
            eprintln!("deltoids hook claude-code: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_claude_code_inner() -> Result<(), String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        // No envelope on stdin: nothing to do. Claude Code only ever
        // invokes this subcommand with JSON piped in.
        return Ok(());
    }

    let mut input = String::new();
    stdin
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    if input.trim().is_empty() {
        return Ok(());
    }

    let envelope: HookEnvelope = serde_json::from_str(&input)
        .map_err(|err| format!("Failed to parse hook envelope: {err}"))?;

    if envelope.hook_event_name != "PostToolUse" {
        // We only act on PostToolUse. Pre and other events would be
        // gracefully ignored if the user wires them up.
        return Ok(());
    }

    let payload = match resolve_payload(&envelope)? {
        Some(payload) => payload,
        None => return Ok(()), // unknown tool: no-op
    };

    let store = TraceStore::from_env()?;
    let resolved = store.resolve_or_create(Some(&envelope.session_id))?;

    record_payload(&store, &resolved.trace_id, &envelope, payload)
}

#[derive(Debug, Deserialize)]
struct HookEnvelope {
    session_id: String,
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    tool_response: serde_json::Value,
}

/// Per-tool view of a `PostToolUse` envelope. `before` is what Claude
/// Code reports as the file content prior to the call; `after` is read
/// from disk so any post-write mutation is captured.
struct ToolPayload {
    tool: &'static str,
    file_path: String,
    before: String,
    after: String,
}

fn resolve_payload(envelope: &HookEnvelope) -> Result<Option<ToolPayload>, String> {
    match envelope.tool_name.as_str() {
        "Write" => Ok(Some(write_payload(envelope)?)),
        "Edit" => Ok(Some(edit_payload(envelope)?)),
        _ => Ok(None),
    }
}

fn write_payload(envelope: &HookEnvelope) -> Result<ToolPayload, String> {
    let file_path = string_field(&envelope.tool_input, "file_path")
        .ok_or_else(|| "Write tool_input missing file_path".to_string())?;
    let after = string_field(&envelope.tool_input, "content")
        .ok_or_else(|| "Write tool_input missing content".to_string())?;
    // For new files, originalFile is `null`; treat as empty.
    let before = original_file(&envelope.tool_response).unwrap_or_default();
    Ok(ToolPayload {
        tool: "write",
        file_path,
        before,
        after,
    })
}

fn edit_payload(envelope: &HookEnvelope) -> Result<ToolPayload, String> {
    let file_path = string_field(&envelope.tool_input, "file_path")
        .ok_or_else(|| "Edit tool_input missing file_path".to_string())?;
    let before = original_file(&envelope.tool_response).unwrap_or_default();
    // Read post-edit content from disk. This captures the actual state
    // after Claude's edit (and any chained PostToolUse mutations from
    // other hooks). If the file was deleted, fall back to an empty
    // after so we still record the trace entry.
    let after = match fs::read_to_string(Path::new(&file_path)) {
        Ok(after) => after,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(format!("Failed to read {file_path}: {err}"));
        }
    };
    Ok(ToolPayload {
        tool: "edit",
        file_path,
        before,
        after,
    })
}

fn string_field(value: &serde_json::Value, field: &str) -> Option<String> {
    value.get(field).and_then(|v| v.as_str()).map(str::to_owned)
}

fn original_file(tool_response: &serde_json::Value) -> Option<String> {
    let value = tool_response.get("originalFile")?;
    if value.is_null() {
        return None;
    }
    value.as_str().map(str::to_owned)
}

fn record_payload(
    store: &TraceStore,
    trace_id: &str,
    envelope: &HookEnvelope,
    payload: ToolPayload,
) -> Result<(), String> {
    let reason = synthesize_reason(&envelope.tool_name);
    let cwd = crate::current_working_directory().unwrap_or_default();
    let timestamp = crate::current_timestamp();

    let computed = deltoids::Diff::compute(&payload.before, &payload.after, &payload.file_path);
    let hunks = computed.hunks().to_vec();
    let diff = computed.text().to_string();
    let language = computed.language();
    let highlight = computed.highlight().map(str::to_string);

    // Reuse the existing `WriteHistoryEntry` shape for both Write and
    // Edit variants: in the Claude Code hook path we always have the
    // full post-edit content but no structured `edits[]` blocks, so a
    // write-shaped entry (with `tool` set to the appropriate label) is
    // the closest match. The TUI's metadata line reads e.g.
    // `edit • ok • 1 hunk`.
    if let Err(error) = store.append(
        trace_id,
        &WriteHistoryEntry {
            v: 2,
            tool: payload.tool,
            trace_id: trace_id.to_string(),
            timestamp: timestamp.clone(),
            cwd: cwd.clone(),
            path: payload.file_path.clone(),
            reason: reason.clone(),
            ok: true,
            content: payload.after.clone(),
            diff,
            hunks,
            language,
            highlight,
        },
    ) {
        // If the success append failed, log a failure entry of the
        // matching variant and surface the original error.
        let failure = match payload.tool {
            "write" => store.append(
                trace_id,
                &WriteFailureHistoryEntry {
                    v: 1,
                    tool: "write",
                    trace_id: trace_id.to_string(),
                    timestamp,
                    cwd,
                    path: payload.file_path,
                    reason,
                    ok: false,
                    content: payload.after,
                    error: error.clone(),
                },
            ),
            _ => store.append(
                trace_id,
                &EditFailureHistoryEntry {
                    v: 1,
                    tool: "edit",
                    trace_id: trace_id.to_string(),
                    timestamp,
                    cwd,
                    path: payload.file_path,
                    reason,
                    ok: false,
                    edits: Vec::new(),
                    error: error.clone(),
                },
            ),
        };
        let _ = failure; // best-effort; we still surface the original error
        return Err(error);
    }

    Ok(())
}

fn synthesize_reason(tool_name: &str) -> String {
    if tool_name.is_empty() {
        "Claude Code".to_string()
    } else {
        format!("Claude Code {tool_name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_reason_with_tool_name() {
        assert_eq!(synthesize_reason("Edit"), "Claude Code Edit");
        assert_eq!(synthesize_reason("Write"), "Claude Code Write");
    }

    #[test]
    fn synthesizes_reason_without_tool_name() {
        assert_eq!(synthesize_reason(""), "Claude Code");
    }

    #[test]
    fn original_file_extracts_string_or_returns_none_for_null() {
        let value = serde_json::json!({"originalFile": "hello\n"});
        assert_eq!(original_file(&value).as_deref(), Some("hello\n"));

        let value = serde_json::json!({"originalFile": null});
        assert_eq!(original_file(&value), None);

        let value = serde_json::json!({});
        assert_eq!(original_file(&value), None);
    }
}
