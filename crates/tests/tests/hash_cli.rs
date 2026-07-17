use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn target_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
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

fn run_command_in_dir(
    subcommand: &str,
    args: &[&str],
    envs: &[(&str, &std::path::Path)],
    input: &[u8],
    current_dir: Option<&std::path::Path>,
) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg(subcommand)
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

/// Extract the `LINEhh` anchor for line `line_1based` from a hashread
/// stdout body whose lines are formatted as `LINEhh|TEXT`.
fn anchor_from_hashread(body: &str, line_1based: usize) -> String {
    let line = body
        .lines()
        .nth(line_1based - 1)
        .unwrap_or_else(|| panic!("hashread output had no line {line_1based}: {body:?}"));
    let pipe = line.find('|').expect("hashread line missing '|' separator");
    line[..pipe].to_string()
}

#[test]
fn hashread_emits_anchored_lines() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("app.txt");
    fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

    let request = serde_json::json!({ "path": path });
    let output = run_command_in_dir(
        "hashread",
        &[],
        &[],
        request.to_string().as_bytes(),
        Some(dir.path()),
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("1") && lines[0].ends_with("|alpha"));
    assert!(lines[1].starts_with("2") && lines[1].ends_with("|beta"));
    assert!(lines[2].starts_with("3") && lines[2].ends_with("|gamma"));
}

#[test]
fn hashread_fails_with_json_error_when_path_missing() {
    let dir = tempdir().unwrap();
    let request = serde_json::json!({ "path": "/no/such/file/zzz" });
    let output = run_command_in_dir(
        "hashread",
        &[],
        &[],
        request.to_string().as_bytes(),
        Some(dir.path()),
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let err: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(err["ok"], false);
    assert!(
        err["error"]
            .as_str()
            .unwrap()
            .contains("Path does not exist")
    );
}

#[test]
fn hashedit_applies_replace_using_anchor_from_hashread() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let path = dir.path().join("app.txt");
    fs::write(&path, "const x = 1;\nconst y = 2;\nconst z = 3;\n").unwrap();

    let read_request = serde_json::json!({ "path": path });
    let read_output = run_command_in_dir(
        "hashread",
        &[],
        &[],
        read_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(read_output.status.success());
    let read_body = String::from_utf8(read_output.stdout).unwrap();
    let anchor = anchor_from_hashread(&read_body, 2);

    let edit_request = serde_json::json!({
        "reason": "Bump y",
        "path": path,
        "edits": [
            {
                "op": "replace",
                "pos": anchor,
                "lines": ["const y = 99;"]
            }
        ]
    });
    let edit_output = run_command_in_dir(
        "hashedit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(
        edit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&edit_output.stderr)
    );

    let response: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    assert_eq!(response["ok"], true);
    assert!(
        response["diff"]
            .as_str()
            .unwrap()
            .contains("+const y = 99;")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "const x = 1;\nconst y = 99;\nconst z = 3;\n"
    );
}

#[test]
fn hashedit_rejects_stale_anchor_and_leaves_file_untouched() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let path = dir.path().join("app.txt");
    fs::write(&path, "const x = 1;\n").unwrap();

    let edit_request = serde_json::json!({
        "reason": "Bump x",
        "path": path,
        "edits": [
            {
                "op": "replace",
                "pos": "1zz",
                "lines": ["const x = 2;"]
            }
        ]
    });
    let edit_output = run_command_in_dir(
        "hashedit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );

    assert!(!edit_output.status.success());
    let err: Value = serde_json::from_slice(&edit_output.stderr).unwrap();
    let message = err["error"].as_str().unwrap();
    assert!(message.contains("Edit rejected"), "{message}");
    // Fresh anchor for line 1 with current content "const x = 1;" must appear.
    assert!(message.contains("|const x = 1;"), "{message}");
    // File must be unchanged.
    assert_eq!(fs::read_to_string(&path).unwrap(), "const x = 1;\n");
}

#[test]
fn hashedit_trace_is_visible_in_traces_subcommand() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let path = dir.path().join("app.txt");
    fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

    let read_output = run_command_in_dir(
        "hashread",
        &[],
        &[],
        serde_json::json!({ "path": path }).to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(read_output.status.success());
    let read_body = String::from_utf8(read_output.stdout).unwrap();
    let anchor = anchor_from_hashread(&read_body, 2);

    let edit_request = serde_json::json!({
        "reason": "Upper beta",
        "path": path,
        "edits": [
            {
                "op": "replace",
                "pos": anchor,
                "lines": ["BETA"]
            }
        ]
    });
    let edit_output = run_command_in_dir(
        "hashedit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(edit_output.status.success());

    let traces_output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(dir.path()),
    );
    assert!(traces_output.status.success());
    let stdout = String::from_utf8(traces_output.stdout).unwrap();
    // The reason from the hashedit entry is visible in the traces TUI.
    // The tool name is no longer surfaced after the traces header rework.
    assert!(stdout.contains("Upper beta"), "{stdout}");
}

#[test]
fn hashedit_ignores_an_unknown_per_op_field() {
    // Ops no longer carry a `reason`; a stray field is ignored rather
    // than failing the whole batch.
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    let path = dir.path().join("app.txt");
    fs::write(&path, "const x = 1;\n").unwrap();

    let read_request = serde_json::json!({ "path": path });
    let read_output = run_command_in_dir(
        "hashread",
        &[],
        &[],
        read_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(read_output.status.success());
    let read_body = String::from_utf8(read_output.stdout).unwrap();
    let anchor = anchor_from_hashread(&read_body, 1);

    let edit_request = serde_json::json!({
        "reason": "Bump x",
        "path": path,
        "edits": [
            {
                "op": "replace",
                "reason": "Bump x to 2",
                "pos": anchor,
                "lines": ["const x = 2;"]
            }
        ]
    });
    let edit_output = run_command_in_dir(
        "hashedit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );

    assert!(
        edit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&edit_output.stderr)
    );
    let json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(fs::read_to_string(&path).unwrap(), "const x = 2;\n");
}
