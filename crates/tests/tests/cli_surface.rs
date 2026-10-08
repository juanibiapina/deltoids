use std::path::PathBuf;
use std::process::{Command, Stdio};

fn deltoids_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join("deltoids")
}

#[test]
fn help_lists_only_pager_and_tui() {
    let output = Command::new(deltoids_binary())
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("  pager"));
    assert!(stdout.contains("  tui"));
    for retired in ["edit", "write", "serve", "hook"] {
        assert!(
            !stdout.contains(&format!("  {retired}")),
            "--help still lists {retired}: {stdout}"
        );
    }
}

#[test]
fn retired_commands_are_rejected() {
    for command in ["hashread", "hashedit", "edit", "write", "serve", "hook"] {
        let output = Command::new(deltoids_binary())
            .arg(command)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{command} unexpectedly succeeded");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("unrecognized subcommand"),
            "unexpected error for {command}: {stderr}"
        );
    }
}

#[test]
fn tui_without_a_terminal_fails_with_a_clear_error() {
    let output = Command::new(deltoids_binary())
        .arg("tui")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("needs a terminal"),
        "unexpected error: {stderr}"
    );
}
