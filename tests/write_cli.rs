use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn write_binary() -> &'static str {
    env!("CARGO_BIN_EXE_write")
}

fn run_write(input: &[u8]) -> Output {
    Command::new(write_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    let diff = json["diff"].as_str().unwrap();
    assert!(diff.contains("-  \"version\": 1"));
    assert!(diff.contains("+  \"version\": 2"));
}
