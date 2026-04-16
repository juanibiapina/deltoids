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
    let mut command = Command::new(binary);
    command
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
        &["history", "list", &trace_id],
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
