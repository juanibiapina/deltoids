//! Integration tests: `deltoids pager` renders symlink changes as the
//! dedicated symlink view instead of crashing with a "missing index
//! blob" error.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::tempdir;

fn deltoids_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join("deltoids")
}

/// Feed `diff` on stdin to `deltoids pager` with icons off, returning the
/// process output.
fn run_pager(diff: &str) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg("pager")
        .env("RV_NO_ICONS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(diff.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

/// ANSI-strip stdout for text assertions. Drops `ESC [ ... m` SGR
/// sequences without pulling in a regex dependency.
fn stdout_plain(output: &Output) -> String {
    let s = String::from_utf8_lossy(&output.stdout);
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        match (in_escape, c) {
            (true, 'm') => in_escape = false,
            (true, _) => {}
            (false, '\u{1b}') => in_escape = true,
            (false, _) => out.push(c),
        }
    }
    out
}

const RETARGET: &str = "diff --git a/link.txt b/link.txt\n\
index 8d14cbf..19acdd8 120000\n\
--- a/link.txt\n\
+++ b/link.txt\n\
@@ -1 +1 @@\n\
-a.txt\n\
\\ No newline at end of file\n\
+b.txt\n\
\\ No newline at end of file\n";

const CREATE: &str = "diff --git a/newlink.txt b/newlink.txt\n\
new file mode 120000\n\
index 0000000..8d14cbf\n\
--- /dev/null\n\
+++ b/newlink.txt\n\
@@ -0,0 +1 @@\n\
+a.txt\n\
\\ No newline at end of file\n";

const DELETE: &str = "diff --git a/newlink.txt b/newlink.txt\n\
deleted file mode 120000\n\
index 8d14cbf..0000000\n\
--- a/newlink.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-a.txt\n\
\\ No newline at end of file\n";

/// Run `git` in `dir`, asserting success.
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.com")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Capture `git diff` output in `dir`.
fn git_diff(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["diff"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Run the pager on `diff` in `dir` (so blob resolution can see the repo).
fn run_pager_in(dir: &Path, diff: &str) -> Output {
    let mut command = Command::new(deltoids_binary());
    command
        .arg("pager")
        .current_dir(dir)
        .env("RV_NO_ICONS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(diff.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

#[test]
fn retargeted_symlink_renders_view_and_exits_zero() {
    let output = run_pager(RETARGET);
    assert!(output.status.success(), "pager should exit 0");
    let text = stdout_plain(&output);
    assert!(text.contains("symlink retargeted"), "got:\n{text}");
    assert!(text.contains("a.txt \u{2192} b.txt"), "got:\n{text}");
    assert!(
        !text.contains("missing index blob"),
        "must not emit missing-blob error, got:\n{text}"
    );
}

#[test]
fn created_symlink_renders_view() {
    let output = run_pager(CREATE);
    assert!(output.status.success());
    let text = stdout_plain(&output);
    assert!(text.contains("symlink created"), "got:\n{text}");
    assert!(text.contains("\u{2192} a.txt"), "got:\n{text}");
}

#[test]
fn deleted_symlink_renders_view() {
    let output = run_pager(DELETE);
    assert!(output.status.success());
    let text = stdout_plain(&output);
    assert!(text.contains("symlink deleted"), "got:\n{text}");
    assert!(text.contains("a.txt \u{2192}"), "got:\n{text}");
}

#[test]
fn file_to_symlink_type_change_renders_symlink_create() {
    // Git splits a type change into a regular delete + a symlink create.
    // The regular half references a real blob, so drive a real repo.
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q"]);
    std::fs::write(path.join("regular.txt"), "regular\n").unwrap();
    std::fs::write(path.join("a.txt"), "hello\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);
    // Replace the regular file with a symlink.
    std::fs::remove_file(path.join("regular.txt")).unwrap();
    std::os::unix::fs::symlink("a.txt", path.join("regular.txt")).unwrap();

    let diff = git_diff(path);
    let output = run_pager_in(path, &diff);
    assert!(output.status.success(), "pager should exit 0");
    let text = stdout_plain(&output);
    // The symlink create half renders as the symlink view.
    assert!(text.contains("symlink created"), "got:\n{text}");
    assert!(
        !text.contains("missing index blob"),
        "must not emit missing-blob error, got:\n{text}"
    );
}
