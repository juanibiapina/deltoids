use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn deltoids_binary() -> &'static str {
    env!("CARGO_BIN_EXE_deltoids")
}

fn run_command(
    subcommand: &str,
    args: &[&str],
    envs: &[(&str, &std::path::Path)],
    input: &[u8],
) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg(subcommand)
        .args(args)
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

#[test]
fn rejects_a_nonexistent_explicit_trace_id_for_edit() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });

    let trace_id = "01JTESTTRACE00000000000000";
    let output = run_command(
        "edit",
        &[trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], format!("Trace does not exist: {trace_id}"));
    assert!(json["traceId"].is_null());
    assert!(json["message"].is_null());

    let trace_path = data_home.path().join("edit").join("traces").join(trace_id);
    assert!(!trace_path.exists());
}

#[test]
fn rejects_a_nonexistent_explicit_trace_id_for_write() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    fs::write(&config_path, "{\n  \"version\": 1\n}\n").unwrap();

    let request = serde_json::json!({
        "reason": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });

    let trace_id = "01JTESTTRACE00000000000000";
    let output = run_command(
        "write",
        &[trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], format!("Trace does not exist: {trace_id}"));
    assert!(json["traceId"].is_null());
    assert!(json["message"].is_null());

    let trace_path = data_home.path().join("edit").join("traces").join(trace_id);
    assert!(!trace_path.exists());
}

#[test]
fn rejects_an_invalid_explicit_trace_id_for_edit() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });

    // Trace ids must be safe directory names. A slash is rejected
    // both because it would escape the trace root and because it is
    // not in the allowed `[A-Za-z0-9_-]` alphabet.
    let trace_id = "bad/trace/id";
    let output = run_command(
        "edit",
        &[trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], format!("Invalid trace id: {trace_id}"));
    assert!(json["traceId"].is_null());
    assert!(json["message"].is_null());
}

#[test]
fn logs_a_failed_reused_edit_to_an_existing_trace() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let first_request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });
    let first_output = run_command(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        first_request.to_string().as_bytes(),
    );
    assert!(first_output.status.success());
    let first_json: Value = serde_json::from_slice(&first_output.stdout).unwrap();
    let trace_id = first_json["traceId"].as_str().unwrap().to_string();

    let second_request = serde_json::json!({
        "reason": "Try a missing edit",
        "path": file_path,
        "oldText": "nope",
        "newText": "yep"
    });
    let second_output = run_command(
        "edit",
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        second_request.to_string().as_bytes(),
    );

    assert!(!second_output.status.success());
    let second_json: Value = serde_json::from_slice(&second_output.stderr).unwrap();
    assert_eq!(second_json["traceId"], trace_id);
    assert_eq!(
        second_json["message"],
        format!("Appended to trace {trace_id}.")
    );

    let history_path = data_home
        .path()
        .join("edit")
        .join("traces")
        .join(&trace_id)
        .join("entries.jsonl");
    let history = fs::read_to_string(history_path).unwrap();
    let entries = history.lines().collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);

    let second: Value = serde_json::from_str(entries[1]).unwrap();
    assert_eq!(second["tool"], "edit");
    assert_eq!(second["ok"], false);
    assert_eq!(second["reason"], "Try a missing edit");
    assert_eq!(second["edits"][0]["reason"], "Try a missing edit");
    assert!(second["error"].as_str().unwrap().contains("Could not find"));
}

#[test]
fn reuses_an_explicit_trace_id_across_edit_and_write() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    let config_path = dir.path().join("config.json");
    fs::write(&file_path, "const x = 1;\n").unwrap();
    fs::write(&config_path, "{\n  \"version\": 1\n}\n").unwrap();

    let edit_request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });
    let edit_output = run_command(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "reason": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command(
        "write",
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
    );

    assert!(write_output.status.success());
    let write_json: Value = serde_json::from_slice(&write_output.stdout).unwrap();
    assert_eq!(write_json["traceId"], trace_id);
    assert_eq!(
        write_json["message"],
        format!("Appended to trace {trace_id}.")
    );

    let history_path = data_home
        .path()
        .join("edit")
        .join("traces")
        .join(&trace_id)
        .join("entries.jsonl");
    let history = fs::read_to_string(history_path).unwrap();
    let entries = history.lines().collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);

    let first: Value = serde_json::from_str(entries[0]).unwrap();
    let second: Value = serde_json::from_str(entries[1]).unwrap();
    assert_eq!(first["tool"], "edit");
    assert_eq!(second["tool"], "write");
    assert_eq!(second["traceId"], trace_id);
    assert_eq!(second["reason"], "Rewrite config");
}
