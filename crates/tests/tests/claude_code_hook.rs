//! Integration tests for `deltoids hook claude-code`.
//!
//! Real Claude Code `PostToolUse` envelopes captured from a live
//! session live under `crates/tests/fixtures/claude-code/`. They
//! cover the two file-mutating tools Claude Code currently ships
//! (`Write` and `Edit`). The tests re-home each fixture's `file_path`
//! into a per-test sandbox,
//! materialise the on-disk state Claude would have left after the
//! call, then pipe the rewritten envelope into the hook subcommand.
//!
//! The hook is supposed to:
//! - record one trace entry per call,
//! - key the trace on `session_id`,
//! - never block (always exit 0 on the happy path), and
//! - never write to stdout.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
}

fn deltoids_binary() -> PathBuf {
    target_dir().join("deltoids")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("claude-code")
}

fn run_hook(envs: &[(&str, &Path)], input: &[u8]) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg("hook")
        .arg("claude-code")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in envs {
        command.env(key, value);
    }

    command
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(input)?;
            child.wait_with_output()
        })
        .unwrap()
}

fn load_fixture(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let bytes = fs::read(&path).expect("fixture should exist");
    serde_json::from_slice(&bytes).expect("fixture should be valid JSON")
}

fn read_entries(data_home: &Path, trace_id: &str) -> Vec<Value> {
    let entries_path = data_home
        .join("edit")
        .join("traces")
        .join(trace_id)
        .join("entries.jsonl");
    let contents = fs::read_to_string(&entries_path).expect("entries.jsonl should exist");
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("entry should parse"))
        .collect()
}

/// Re-home a captured envelope's `file_path` into `sandbox`, materialise
/// the on-disk state Claude would have left after the call, and return
/// the rewritten envelope as bytes. For Write, the post-call disk state
/// is `tool_input.content`; for Edit it's reconstructed by applying
/// `tool_input.new_string` against `tool_response.originalFile`.
fn rehome_envelope(mut envelope: Value, sandbox: &Path) -> (Value, PathBuf) {
    let original_path = envelope["tool_input"]["file_path"]
        .as_str()
        .expect("fixture must carry a file_path")
        .to_string();
    let file_name = Path::new(&original_path)
        .file_name()
        .expect("file_path has a basename");
    let new_path = sandbox.join(file_name);

    envelope["tool_input"]["file_path"] = Value::String(new_path.to_string_lossy().into_owned());
    if envelope["tool_response"].is_object() {
        envelope["tool_response"]["filePath"] =
            Value::String(new_path.to_string_lossy().into_owned());
    }

    // Reconstruct what Claude would have written to disk for this call.
    let on_disk = match envelope["tool_name"].as_str().unwrap_or("") {
        "Write" => envelope["tool_input"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "Edit" => {
            let original = envelope["tool_response"]["originalFile"]
                .as_str()
                .unwrap_or_default();
            let old = envelope["tool_input"]["old_string"].as_str().unwrap_or("");
            let new = envelope["tool_input"]["new_string"].as_str().unwrap_or("");
            original.replacen(old, new, 1)
        }
        _ => String::new(),
    };
    fs::write(&new_path, on_disk).expect("write sandbox file");

    (envelope, new_path)
}

fn write_input(envelope: &Value) -> Vec<u8> {
    serde_json::to_vec(envelope).expect("envelope serializes")
}

#[test]
fn records_a_write_create_under_the_session_trace() {
    let sandbox = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let envelope = load_fixture("post-write-create.json");
    let session_id = envelope["session_id"].as_str().unwrap().to_string();

    let (envelope, file_path) = rehome_envelope(envelope, sandbox.path());

    let output = run_hook(
        &[("XDG_DATA_HOME", data_home.path())],
        &write_input(&envelope),
    );

    assert!(
        output.status.success(),
        "hook should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "hook must not write to stdout (would pollute the user's transcript): {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    let entries = read_entries(data_home.path(), &session_id);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["tool"], "write");
    assert_eq!(entry["ok"], true);
    assert_eq!(entry["reason"], "Claude Code Write");
    assert_eq!(entry["path"], file_path.to_string_lossy().into_owned());
    assert_eq!(entry["traceId"], session_id);
    assert!(entry["diff"].as_str().unwrap().contains("+def greet"));
    assert!(!entry["hunks"].as_array().unwrap().is_empty());
}

#[test]
fn records_an_edit_under_the_same_session_trace() {
    let sandbox = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let envelope = load_fixture("post-edit-1.json");
    let session_id = envelope["session_id"].as_str().unwrap().to_string();

    let (envelope, file_path) = rehome_envelope(envelope, sandbox.path());

    let output = run_hook(
        &[("XDG_DATA_HOME", data_home.path())],
        &write_input(&envelope),
    );

    assert!(output.status.success());
    let entries = read_entries(data_home.path(), &session_id);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["tool"], "edit");
    assert_eq!(entry["reason"], "Claude Code Edit");
    assert_eq!(entry["path"], file_path.to_string_lossy().into_owned());
    let diff = entry["diff"].as_str().unwrap();
    assert!(diff.contains("-    return f\"hi {name}\""));
    assert!(diff.contains("+    return f\"hello {name}\""));
}

#[test]
fn appends_subsequent_edits_to_the_existing_session_trace() {
    let sandbox = tempdir().unwrap();
    let data_home = tempdir().unwrap();

    // Replay all three captured envelopes in order: write the file,
    // then two edits. They share `session_id`, so they all end up in
    // the same trace.
    let session_id = load_fixture("post-write-create.json")["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    for fixture in [
        "post-write-create.json",
        "post-edit-1.json",
        "post-edit-2.json",
    ] {
        let envelope = load_fixture(fixture);
        let (envelope, _) = rehome_envelope(envelope, sandbox.path());
        let output = run_hook(
            &[("XDG_DATA_HOME", data_home.path())],
            &write_input(&envelope),
        );
        assert!(
            output.status.success(),
            "fixture {fixture} should succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let entries = read_entries(data_home.path(), &session_id);
    assert_eq!(entries.len(), 3, "three calls => three trace entries");
    let tools: Vec<&str> = entries
        .iter()
        .map(|entry| entry["tool"].as_str().unwrap())
        .collect();
    assert_eq!(tools, vec!["write", "edit", "edit"]);
}

#[test]
fn unknown_tool_is_a_no_op() {
    let data_home = tempdir().unwrap();
    let envelope = serde_json::json!({
        "session_id": "40cc627a-e96a-41bb-8259-ae81589f5599",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
        "tool_response": {"stdout": "", "stderr": "", "exit_code": 0}
    });

    let output = run_hook(
        &[("XDG_DATA_HOME", data_home.path())],
        &write_input(&envelope),
    );

    assert!(output.status.success());
    let traces_dir = data_home.path().join("edit").join("traces");
    assert!(
        !traces_dir.exists() || fs::read_dir(&traces_dir).unwrap().next().is_none(),
        "no trace should be created for unknown tools"
    );
}

#[test]
fn pre_tool_use_event_is_a_no_op() {
    let data_home = tempdir().unwrap();
    let envelope = serde_json::json!({
        "session_id": "40cc627a-e96a-41bb-8259-ae81589f5599",
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "/does/not/matter",
            "old_string": "x",
            "new_string": "y"
        }
    });

    let output = run_hook(
        &[("XDG_DATA_HOME", data_home.path())],
        &write_input(&envelope),
    );

    assert!(output.status.success());
    let traces_dir = data_home.path().join("edit").join("traces");
    assert!(
        !traces_dir.exists() || fs::read_dir(&traces_dir).unwrap().next().is_none(),
        "PreToolUse should not record a trace entry"
    );
}

#[test]
fn malformed_envelope_exits_one_without_blocking() {
    let data_home = tempdir().unwrap();

    let output = run_hook(&[("XDG_DATA_HOME", data_home.path())], b"not json");

    assert_eq!(
        output.status.code(),
        Some(1),
        "should exit 1 (non-blocking), not 2 (blocking)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to parse hook envelope"),
        "stderr should explain the failure: {stderr}"
    );
}

#[test]
fn empty_stdin_is_a_no_op() {
    let data_home = tempdir().unwrap();

    let output = run_hook(&[("XDG_DATA_HOME", data_home.path())], b"");

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}
