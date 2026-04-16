use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn write_binary() -> &'static str {
    env!("CARGO_BIN_EXE_write")
}

fn run_write(input: &[u8]) -> Output {
    run_write_with_env(&[], input)
}

fn run_write_with_env(envs: &[(&str, &std::path::Path)], input: &[u8]) -> Output {
    let mut command = Command::new(write_binary());
    command
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
fn rewrites_a_file_from_stdin_json() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("config.json");
    fs::write(&file_path, "{\n  \"version\": 1\n}\n").unwrap();

    let request = serde_json::json!({
        "summary": "Rewrite config",
        "path": file_path,
        "content": "{\n  \"version\": 2\n}\n"
    });

    let output = run_write(request.to_string().as_bytes());

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&file_path).unwrap(),
        "{\n  \"version\": 2\n}\n"
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert!(json["traceId"].as_str().unwrap().len() >= 10);
    assert!(json["message"].as_str().unwrap().contains("Started trace"));
    let diff = json["diff"].as_str().unwrap();
    assert!(diff.contains("-  \"version\": 1"));
    assert!(diff.contains("+  \"version\": 2"));
}

#[test]
fn starts_a_trace_and_logs_successful_writes() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let file_path = dir.path().join("config.json");
    fs::write(&file_path, "{\n  \"version\": 1\n}\n").unwrap();

    let request = serde_json::json!({
        "summary": "Rewrite config",
        "path": file_path,
        "content": "{\n  \"version\": 2\n}\n"
    });

    let output = run_write_with_env(
        &[("XDG_DATA_HOME", data_home.path())],
        request.to_string().as_bytes(),
    );

    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let trace_id = json["traceId"].as_str().unwrap();
    let trace_path = data_home
        .path()
        .join("edit")
        .join("traces")
        .join(trace_id)
        .join("entries.jsonl");
    let history = fs::read_to_string(trace_path).unwrap();
    let entry: Value = serde_json::from_str(history.lines().next().unwrap()).unwrap();
    assert_eq!(entry["tool"], "write");
    assert_eq!(entry["traceId"], trace_id);
    assert_eq!(entry["ok"], true);
    assert_eq!(entry["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(entry["summary"], "Rewrite config");
    assert_eq!(entry["content"], "{\n  \"version\": 2\n}\n");
    assert!(
        entry["diff"]
            .as_str()
            .unwrap()
            .contains("+  \"version\": 2")
    );
}
