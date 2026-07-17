use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn deltoids_binary() -> &'static str {
    env!("CARGO_BIN_EXE_deltoids")
}

fn run_edit(input: &[u8]) -> Output {
    run_edit_with_args_and_env(&[], &[], input)
}

fn run_edit_with_args(args: &[&str], input: &[u8]) -> Output {
    run_edit_with_args_and_env(args, &[], input)
}

fn run_edit_with_args_and_env(
    args: &[&str],
    envs: &[(&str, &std::path::Path)],
    input: &[u8],
) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg("edit")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let data_home_fallback = if envs.iter().any(|(k, _)| *k == "XDG_DATA_HOME") {
        None
    } else {
        Some(tempdir().unwrap())
    };
    if let Some(ref data_home) = data_home_fallback {
        command.env("XDG_DATA_HOME", data_home.path());
    }

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
fn starts_a_trace_and_logs_successful_edits() {
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

    let output = run_edit_with_args_and_env(
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "const x = 2;\n");

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let trace_id = json["traceId"].as_str().unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert!(json["message"].as_str().unwrap().contains(trace_id));
    let diff = json["diff"].as_str().unwrap();
    assert!(diff.contains("-const x = 1;"));
    assert!(diff.contains("+const x = 2;"));

    let trace_path = data_home
        .path()
        .join("edit")
        .join("traces")
        .join(trace_id)
        .join("entries.jsonl");
    let history = fs::read_to_string(trace_path).unwrap();
    let entry: Value = serde_json::from_str(history.lines().next().unwrap()).unwrap();
    assert_eq!(entry["tool"], "edit");
    assert_eq!(entry["traceId"], trace_id);
    assert_eq!(entry["ok"], true);
    assert_eq!(entry["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(entry["reason"], "Update x constant");
    // The single edit records one trace edit whose reason mirrors the
    // top-level reason.
    assert_eq!(entry["edits"][0]["reason"], "Update x constant");
    assert!(entry["diff"].as_str().unwrap().contains("+const x = 2;"));
}

#[test]
fn edits_a_file_from_stdin_json() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "const x = 2;\n");

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert!(json["traceId"].as_str().unwrap().len() >= 10);
    assert!(json["message"].as_str().unwrap().contains("Started trace"));
    let diff = json["diff"].as_str().unwrap();
    assert!(diff.contains("-const x = 1;"));
    assert!(diff.contains("+const x = 2;"));
}

#[test]
fn logs_failed_edits_and_returns_trace_id() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("missing.txt");
    let original = "hello\nworld\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": "Try a missing edit",
        "path": file_path,
        "oldText": "nope",
        "newText": "yep"
    });

    let output = run_edit_with_args_and_env(
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    let trace_id = json["traceId"].as_str().unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["message"].as_str().unwrap().contains(trace_id));
    assert!(json["error"].as_str().unwrap().contains("Could not find"));

    let trace_path = data_home
        .path()
        .join("edit")
        .join("traces")
        .join(trace_id)
        .join("entries.jsonl");
    let history = fs::read_to_string(trace_path).unwrap();
    let entry: Value = serde_json::from_str(history.lines().next().unwrap()).unwrap();
    assert_eq!(entry["tool"], "edit");
    assert_eq!(entry["traceId"], trace_id);
    assert_eq!(entry["ok"], false);
    assert_eq!(entry["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(entry["reason"], "Try a missing edit");
    assert_eq!(entry["edits"][0]["reason"], "Try a missing edit");
    assert!(entry["error"].as_str().unwrap().contains("Could not find"));
}

#[test]
fn fails_without_changing_the_file_when_text_is_missing() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("missing.txt");
    let original = "hello\nworld\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": "Try a missing edit",
        "path": file_path,
        "oldText": "nope",
        "newText": "yep"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("Could not find"));
}

#[test]
fn edits_a_file_with_shorthand_flags() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("shorthand.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let output = run_edit_with_args(
        &[
            "--path",
            file_path.to_string_lossy().as_ref(),
            "--reason",
            "Update x constant",
            "--old",
            "const x = 1;",
            "--new",
            "const x = 2;",
        ],
        b"",
    );

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "const x = 2;\n");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
}

#[test]
fn shows_overview_when_stdin_is_empty() {
    let output = run_edit(b"");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("CLI for agents to edit files."));
    assert!(stdout.contains("reason"));
    assert!(stdout.contains("oldText"));
    assert!(stdout.contains("newText"));
    assert!(stdout.contains("printf '%s'"));
    assert!(output.stderr.is_empty());
}

#[test]
fn shows_agent_friendly_help_with_help_flag() {
    let output = run_edit_with_args(&["--help"], b"");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: deltoids edit"));
    assert!(stdout.contains("CLI for agents to edit files."));
    assert!(stdout.contains("oldText"));
    assert!(stdout.contains("newText"));
    assert!(stdout.contains("printf '%s'"));
    assert!(output.stderr.is_empty());
}

#[test]
fn shows_overview_when_stdin_is_whitespace_only() {
    let output = run_edit(b" \n\t ");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("CLI for agents to edit files."));
    assert!(stdout.contains("reason"));
    assert!(stdout.contains("oldText"));
    assert!(stdout.contains("newText"));
    assert!(output.stderr.is_empty());
}

#[test]
fn fails_on_invalid_json() {
    let output = run_edit(b"not json");

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Invalid request JSON")
    );
}

#[test]
fn fails_when_old_text_is_empty() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("empty-old-text.txt");
    let original = "hello\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": "Reject empty oldText",
        "path": file_path,
        "oldText": "",
        "newText": "world"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("oldText must not be empty")
    );
}

#[test]
fn fails_when_request_is_missing_path() {
    let request = serde_json::json!({
        "reason": "Missing path",
        "oldText": "a",
        "newText": "b"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Invalid request JSON")
    );
}

#[test]
fn fails_when_target_path_does_not_exist() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("missing.txt");

    let request = serde_json::json!({
        "reason": "Missing target file",
        "path": file_path,
        "oldText": "a",
        "newText": "b"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(
        json["error"].as_str().unwrap(),
        format!("Path does not exist: {}", file_path.to_string_lossy())
    );
}

#[test]
fn fails_when_target_path_is_a_directory() {
    let dir = tempdir().unwrap();

    let request = serde_json::json!({
        "reason": "Directory target",
        "path": dir.path(),
        "oldText": "a",
        "newText": "b"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(
        json["error"].as_str().unwrap(),
        format!("Path is not a file: {}", dir.path().to_string_lossy())
    );
}

#[test]
fn ignores_unknown_top_level_field() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("extra-field.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let request = serde_json::json!({
        "reason": "Unknown top-level field",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;",
        "extra": true
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "const x = 2;\n");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
}

#[test]
fn fails_when_edit_uses_snake_case_keys() {
    let request = serde_json::json!({
        "reason": "Snake case keys",
        "path": "file.txt",
        "old_text": "a",
        "new_text": "b"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Invalid request JSON")
    );
}

#[test]
fn fails_when_match_is_duplicated() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("duplicate.txt");
    let original = "foo foo foo\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": "Duplicate match",
        "path": file_path,
        "oldText": "foo",
        "newText": "bar"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("The oldText must be unique")
    );
}

#[test]
fn fails_when_request_is_missing_reason() {
    let request = serde_json::json!({
        "path": "file.txt",
        "oldText": "a",
        "newText": "b"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Invalid request JSON")
    );
}

#[test]
fn fails_when_reason_is_empty() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("empty-reason.txt");
    let original = "hello\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": "",
        "path": file_path,
        "oldText": "hello",
        "newText": "world"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("reason must not be empty")
    );
}

#[test]
fn fails_when_reason_is_whitespace_only() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("whitespace-reason.txt");
    let original = "hello\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "reason": " \n\t ",
        "path": file_path,
        "oldText": "hello",
        "newText": "world"
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("reason must not be empty")
    );
}
