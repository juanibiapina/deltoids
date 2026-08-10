use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn target_dir() -> PathBuf {
    // Find workspace target directory
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

#[test]
fn renders_empty_state_for_directory_with_no_traces() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();

    let output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(dir.path()),
    );

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("No traces"));
}

#[test]
fn renders_traces_and_entries_for_current_directory() {
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
    let edit_output = run_command_in_dir(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "reason": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command_in_dir(
        "write",
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(write_output.status.success());

    let tui_output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(dir.path()),
    );

    assert!(tui_output.status.success());
    let stdout = String::from_utf8(tui_output.stdout).unwrap();
    assert!(stdout.contains("[1] Entries 1 of 2"));
    assert!(stdout.contains("[2] Traces 1 of 1"));
    // Entries list shows each entry's reason.
    assert!(stdout.contains("\u{2713} Update x constant"));
    assert!(stdout.contains("\u{2713} Rewrite config"));
    assert!(stdout.contains(&trace_id[..10]));
    // Detail header shows the selected entry's path.
    assert!(stdout.contains("app.txt"));
}

#[test]
fn j_navigates_entries_by_default_then_tab_switches_to_traces() {
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
    let edit_output = run_command_in_dir(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(edit_output.status.success());
    let edit_json: Value = serde_json::from_slice(&edit_output.stdout).unwrap();
    let trace_id = edit_json["traceId"].as_str().unwrap().to_string();

    let write_request = serde_json::json!({
        "reason": "Rewrite config",
        "path": config_path,
        "content": "{\n  \"version\": 2\n}\n"
    });
    let write_output = run_command_in_dir(
        "write",
        &[&trace_id],
        &[("XDG_DATA_HOME", data_home.path())],
        write_request.to_string().as_bytes(),
        Some(dir.path()),
    );
    assert!(write_output.status.success());

    // Entries pane is focused by default, so `j` moves the entry selection
    // straight to the second entry (write). `\t` then proves Tab switches
    // focus to the traces pane.
    let tui_output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"j\t",
        Some(dir.path()),
    );

    assert!(tui_output.status.success());
    let stdout = String::from_utf8(tui_output.stdout).unwrap();
    // The entries pane lists each entry by its reason, marked selected.
    assert!(stdout.contains("> \u{2713} Rewrite config"));
    // The detail header shows the selected entry's path.
    assert!(stdout.contains("config.json"));
    assert!(stdout.contains("\"version\": 2"));
    assert!(stdout.contains("* [2] Traces"));
}

#[test]
fn comments_on_diff_lines_copy_out_as_a_review_prompt() {
    let dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();
    // Canonicalize so the recorded path and the process cwd agree (macOS
    // temp dirs live behind a /var -> /private/var symlink), which is what
    // lets the prompt show a working-directory-relative path.
    let work_dir = fs::canonicalize(dir.path()).unwrap();
    let file_path = work_dir.join("app.txt");
    fs::write(&file_path, "line one\nline two\nconst x = 1;\nline four\n").unwrap();

    let edit_request = serde_json::json!({
        "reason": "Update x constant",
        "path": file_path,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });
    let edit_output = run_command_in_dir(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        edit_request.to_string().as_bytes(),
        Some(&work_dir),
    );
    assert!(edit_output.status.success());

    // Focus the diff pane (`3`), step down to the removed line (`jj`),
    // comment on it, step to the added line, comment on it, then copy
    // (`y`). Without a terminal the prompt is printed instead of copied.
    let tui_output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"3jjcwhy removed\njckeep this\ny",
        Some(&work_dir),
    );

    assert!(tui_output.status.success());
    let stdout = String::from_utf8(tui_output.stdout).unwrap();
    assert!(stdout.contains("Address the following code review comments"));
    assert!(stdout.contains(
        "A line marked outdated is quoted as it read when the note was written; it has changed since, so locate the reviewer's intent rather than trusting the number."
    ));
    // Removed line: old-file numbering and a `-` marker.
    assert!(
        stdout.contains("app.txt:3\n- const x = 1;\nnote: why removed"),
        "prompt was:\n{stdout}"
    );
    // Added line: new-file numbering and a `+` marker.
    assert!(
        stdout.contains("app.txt:3\n+ const x = 2;\nnote: keep this"),
        "prompt was:\n{stdout}"
    );
    // The comments also render inline under their diff lines.
    assert!(stdout.contains("why removed"));
    assert!(stdout.contains("keep this"));
}

#[test]
fn shows_only_traces_for_the_current_directory() {
    let first_dir = tempdir().unwrap();
    let second_dir = tempdir().unwrap();
    let data_home = tempdir().unwrap();

    let first_file = first_dir.path().join("app.txt");
    fs::write(&first_file, "const x = 1;\n").unwrap();
    let first_request = serde_json::json!({
        "reason": "Update first app",
        "path": first_file,
        "oldText": "const x = 1;",
        "newText": "const x = 2;"
    });
    let first_output = run_command_in_dir(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        first_request.to_string().as_bytes(),
        Some(first_dir.path()),
    );
    assert!(first_output.status.success());
    let first_trace_id = serde_json::from_slice::<Value>(&first_output.stdout).unwrap()["traceId"]
        .as_str()
        .unwrap()
        .to_string();

    let second_file = second_dir.path().join("app.txt");
    fs::write(&second_file, "const y = 1;\n").unwrap();
    let second_request = serde_json::json!({
        "reason": "Update second app",
        "path": second_file,
        "oldText": "const y = 1;",
        "newText": "const y = 2;"
    });
    let second_output = run_command_in_dir(
        "edit",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        second_request.to_string().as_bytes(),
        Some(second_dir.path()),
    );
    assert!(second_output.status.success());
    let second_trace_id =
        serde_json::from_slice::<Value>(&second_output.stdout).unwrap()["traceId"]
            .as_str()
            .unwrap()
            .to_string();

    let tui_output = run_command_in_dir(
        "tui",
        &[],
        &[("XDG_DATA_HOME", data_home.path())],
        b"",
        Some(first_dir.path()),
    );

    assert!(tui_output.status.success());
    let stdout = String::from_utf8(tui_output.stdout).unwrap();
    assert!(stdout.contains(&first_trace_id[..10]));
    assert!(!stdout.contains(&second_trace_id[..10]));
}
