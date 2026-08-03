use std::path::PathBuf;
use std::process::Command;

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
fn help_lists_only_supported_agent_tools() {
    let output = Command::new(deltoids_binary())
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("  edit"));
    assert!(stdout.contains("  write"));
    assert!(!stdout.contains("hashread"));
    assert!(!stdout.contains("hashedit"));
}

#[test]
fn retired_hash_commands_are_rejected() {
    for command in ["hashread", "hashedit"] {
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
