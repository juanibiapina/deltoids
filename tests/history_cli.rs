use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn edit_binary() -> &'static str {
    env!("CARGO_BIN_EXE_edit")
}

fn write_binary() -> &'static str {
    env!("CARGO_BIN_EXE_write")
}

fn run_command(
    binary: &str,
    args: &[&str],
    envs: &[(&str, &std::path::Path)],
    input: &[u8],
) -> Output {
    run_command_in_dir(binary, args, envs, input, None)
}

fn run_command_in_dir(
    binary: &str,
    args: &[&str],
    envs: &[(&str, &std::path::Path)],
    input: &[u8],
    current_dir: Option<&std::path::Path>,
) -> Output {
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
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
fn shows_one_trace_history_entry() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    let config_path = dir.path().join("config.json");
    fs::write(&file_path, "const x = 1;\n").unwrap();
    fs::write(&config_path, "{\n  \"version\": 1\n}\n").unwrap();

    let edit_request = serde_json::json!({
        "summary": "Update x constant",
        "path": file_path,
        "edits": [
            {
                "summary": "Edit change",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let edit_output = run_command(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "summary": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command(
        write_binary(),
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
    );
    assert!(write_output.status.success());

    let show_output = run_command(
        edit_binary(),
        &["traces", "show", &trace_id, "2"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(show_output.status.success());
    let stdout = String::from_utf8(show_output.stdout).unwrap();
    assert!(stdout.contains("tool: write"));
    assert!(stdout.contains("summary: Rewrite config"));
    assert!(stdout.contains("diff:"));
    assert!(stdout.contains("-  \"version\": 1"));
    assert!(stdout.contains("+  \"version\": 2"));
}

#[test]
fn reviews_trace_history_entries() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    let config_path = dir.path().join("config.json");
    fs::write(&file_path, "const x = 1;\n").unwrap();
    fs::write(&config_path, "{\n  \"version\": 1\n}\n").unwrap();

    let edit_request = serde_json::json!({
        "summary": "Update x constant",
        "path": file_path,
        "edits": [
            {
                "summary": "Edit change",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let edit_output = run_command(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "summary": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command(
        write_binary(),
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
    );
    assert!(write_output.status.success());

    let review_output = run_command(
        edit_binary(),
        &["traces", "review", &trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        b"jq",
    );

    assert!(review_output.status.success());
    let stdout = String::from_utf8(review_output.stdout).unwrap();
    assert!(stdout.contains("1 edit ok"));
    assert!(stdout.contains("2 write ok"));
    assert!(stdout.contains("tool: write"));
    assert!(stdout.contains("summary: Rewrite config"));
    assert!(stdout.contains("+  \"version\": 2"));
}

#[test]
fn rejects_an_invalid_trace_id_for_history_list() {
    let data_home = tempdir().unwrap();
    let trace_id = "../bad-trace-id";

    let output = run_command(
        edit_binary(),
        &["traces", "list", trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], format!("Invalid trace id: {trace_id}"));
}

#[test]
fn rejects_a_nonexistent_trace_id_for_history_show() {
    let data_home = tempdir().unwrap();
    let trace_id = "01JTESTTRACE00000000000000";

    let output = run_command(
        edit_binary(),
        &["traces", "show", trace_id, "1"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], format!("Trace not found: {trace_id}"));
}

#[test]
fn lists_trace_history_entries() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    let config_path = dir.path().join("config.json");
    fs::write(&file_path, "const x = 1;\n").unwrap();
    fs::write(&config_path, "{\n  \"version\": 1\n}\n").unwrap();

    let edit_request = serde_json::json!({
        "summary": "Update x constant",
        "path": file_path,
        "edits": [
            {
                "summary": "Edit change",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let edit_output = run_command(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "summary": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command(
        write_binary(),
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
    );
    assert!(write_output.status.success());

    let list_output = run_command(
        edit_binary(),
        &["traces", "list", &trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(list_output.status.success());
    let stdout = String::from_utf8(list_output.stdout).unwrap();
    assert!(stdout.contains("1"));
    assert!(stdout.contains("edit"));
    assert!(stdout.contains("ok"));
    assert!(stdout.contains(file_path.to_string_lossy().as_ref()));
    assert!(stdout.contains("Update x constant"));
    assert!(stdout.contains("2"));
    assert!(stdout.contains("write"));
    assert!(stdout.contains(config_path.to_string_lossy().as_ref()));
    assert!(stdout.contains("Rewrite config"));
}

#[test]
fn lists_traces_for_the_current_directory_only() {
    let first_dir = tempdir().unwrap();
    let second_dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();

    let first_file = first_dir.path().join("app.txt");
    fs::write(&first_file, "const x = 1;\n").unwrap();
    let first_request = serde_json::json!({
        "summary": "Update first app",
        "path": first_file,
        "edits": [
            {
                "summary": "Edit first app",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let first_output = run_command_in_dir(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        first_request.to_string().as_bytes(),
        Some(first_dir.path()),
    );
    assert!(first_output.status.success());
    let first_json: Value = serde_json::from_slice(&first_output.stdout).unwrap();
    let first_trace_id = first_json["traceId"].as_str().unwrap().to_string();

    let second_file = second_dir.path().join("app.txt");
    fs::write(&second_file, "const y = 1;\n").unwrap();
    let second_request = serde_json::json!({
        "summary": "Update second app",
        "path": second_file,
        "edits": [
            {
                "summary": "Edit second app",
                "oldText": "const y = 1;",
                "newText": "const y = 2;"
            }
        ]
    });
    let second_output = run_command_in_dir(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        second_request.to_string().as_bytes(),
        Some(second_dir.path()),
    );
    assert!(second_output.status.success());
    let second_json: Value = serde_json::from_slice(&second_output.stdout).unwrap();
    let second_trace_id = second_json["traceId"].as_str().unwrap().to_string();

    let list_output = run_command_in_dir(
        edit_binary(),
        &["traces", "list"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(first_dir.path()),
    );

    assert!(list_output.status.success());
    let stdout = String::from_utf8(list_output.stdout).unwrap();
    assert!(stdout.contains(&first_trace_id));
    assert!(!stdout.contains(&second_trace_id));
}

#[test]
fn prints_nothing_when_no_traces_match_the_current_directory() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();

    let output = run_command_in_dir(
        edit_binary(),
        &["traces", "list"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(dir.path()),
    );

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
}

#[test]
fn rejects_the_old_history_commands() {
    let data_home = tempdir().unwrap();

    let output = run_command(
        edit_binary(),
        &["history", "list", "01JTESTTRACE00000000000000"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("edit traces list"));
}

#[test]
fn rejects_the_old_trace_commands() {
    let data_home = tempdir().unwrap();

    let output = run_command(
        edit_binary(),
        &["trace", "01JTESTTRACE00000000000000", "list"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("edit traces list"));
}

#[test]
fn rejects_an_invalid_trace_entry_index() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let edit_request = serde_json::json!({
        "summary": "Update x constant",
        "path": file_path,
        "edits": [
            {
                "summary": "Edit change",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let edit_output = run_command(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let output = run_command(
        edit_binary(),
        &["traces", "show", &trace_id, "bad"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "Invalid trace entry index: bad");
}

#[test]
fn rejects_an_out_of_range_trace_entry_index() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let edit_request = serde_json::json!({
        "summary": "Update x constant",
        "path": file_path,
        "edits": [
            {
                "summary": "Edit change",
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });
    let edit_output = run_command(
        edit_binary(),
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let output = run_command(
        edit_binary(),
        &["traces", "show", &trace_id, "2"],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
    );

    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "Trace entry index out of range: 2");
}
