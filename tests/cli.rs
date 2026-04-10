use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::tempdir;

fn edit_binary() -> &'static str {
    env!("CARGO_BIN_EXE_edit")
}

fn run_edit(input: &[u8]) -> Output {
    Command::new(edit_binary())
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
fn edits_a_file_from_stdin_json() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("app.txt");
    fs::write(&file_path, "const x = 1;\n").unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            {
                "oldText": "const x = 1;",
                "newText": "const x = 2;"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "const x = 2;\n");

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(json["replacedBlocks"], 1);
}

#[test]
fn applies_multiple_edits_in_one_invocation() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("multi.txt");
    fs::write(&file_path, "alpha\nbeta\ngamma\ndelta\n").unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            { "oldText": "alpha\n", "newText": "ALPHA\n" },
            { "oldText": "gamma\n", "newText": "GAMMA\n" }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&file_path).unwrap(),
        "ALPHA\nbeta\nGAMMA\ndelta\n"
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(json["replacedBlocks"], 2);
}

#[test]
fn fails_without_changing_the_file_when_text_is_missing() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("missing.txt");
    let original = "hello\nworld\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            {
                "oldText": "nope",
                "newText": "yep"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("Could not find"));
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
fn fails_when_edits_is_empty() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("empty-edits.txt");
    let original = "hello\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": []
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("edits must contain at least one replacement")
    );
}

#[test]
fn fails_when_old_text_is_empty() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("empty-old-text.txt");
    let original = "hello\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            {
                "oldText": "",
                "newText": "world"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("edits[0].oldText must not be empty")
    );
}

#[test]
fn fails_when_request_is_missing_path() {
    let request = serde_json::json!({
        "edits": [
            {
                "oldText": "a",
                "newText": "b"
            }
        ]
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
fn fails_when_request_has_unknown_field() {
    let request = serde_json::json!({
        "path": "file.txt",
        "edits": [],
        "extra": true
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
fn fails_when_edit_uses_snake_case_keys() {
    let request = serde_json::json!({
        "path": "file.txt",
        "edits": [
            {
                "old_text": "a",
                "new_text": "b"
            }
        ]
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
        "path": file_path,
        "edits": [
            {
                "oldText": "foo",
                "newText": "bar"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Each oldText must be unique")
    );
}

#[test]
fn fails_when_edits_overlap() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("overlap.txt");
    let original = "one\ntwo\nthree\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            {
                "oldText": "one\ntwo\n",
                "newText": "ONE\nTWO\n"
            },
            {
                "oldText": "two\nthree\n",
                "newText": "TWO\nTHREE\n"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(json["error"].as_str().unwrap().contains("overlap"));
}

#[test]
fn does_not_partially_apply_multi_edit_when_one_edit_fails() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("no-partial-multi.txt");
    let original = "alpha\nbeta\ngamma\n";
    fs::write(&file_path, original).unwrap();

    let request = serde_json::json!({
        "path": file_path,
        "edits": [
            {
                "oldText": "alpha\n",
                "newText": "ALPHA\n"
            },
            {
                "oldText": "missing\n",
                "newText": "MISSING\n"
            }
        ]
    });

    let output = run_edit(request.to_string().as_bytes());

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(json["error"].as_str().unwrap().contains("Could not find"));
}
