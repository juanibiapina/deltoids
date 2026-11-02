//! Integration tests for index-blob resolution.
//!
//! `deltoids-cli` must:
//!
//!   * Fail with a clear error when the diff references an index blob that
//!     is neither in the local git ODB nor reproducible from the working
//!     tree (e.g. piping `gh pr diff` for a PR branch you haven't fetched).
//!   * Succeed for working-tree `git diff`, where the new index hash is
//!     synthetic (not in the ODB) but the filesystem holds the matching
//!     content.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::tempdir;

/// Run `git args` in `dir` with `input` on stdin, asserting success, and
/// return its trimmed stdout.
fn git(dir: &Path, args: &[&str], input: &str) -> String {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Initialise a git repo at `path` containing a single committed file.
/// Returns the blob hash for the committed file.
fn init_repo_with_file(path: &Path, file_name: &str, content: &str) -> String {
    git(path, &["init", "-q"], "");
    std::fs::write(path.join(file_name), content).unwrap();
    git(path, &["add", file_name], "");
    git(path, &["commit", "-q", "-m", "init"], "");
    git(path, &["rev-parse", &format!("HEAD:{file_name}")], "")
}

fn run_deltoids(cwd: &Path, stdin: &str) -> Output {
    let bin = env!("CARGO_BIN_EXE_deltoids");
    let mut child = Command::new(bin)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn fails_with_clear_error_when_index_blob_is_missing() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    let old_hash = init_repo_with_file(path, "hello.txt", "hello\nworld\n");

    // A plausible-looking new hash that is *not* in the ODB and that the
    // working tree (still equal to the committed content) does not hash to.
    let new_hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let diff = format!(
        "diff --git a/hello.txt b/hello.txt\n\
         index {old_hash}..{new_hash} 100644\n\
         --- a/hello.txt\n\
         +++ b/hello.txt\n\
         @@ -1,2 +1,2 @@\n\
         -hello\n\
         +HELLO\n\
          world\n",
    );

    let output = run_deltoids(path, &diff);

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status,
    );
    assert!(
        output.stdout.is_empty(),
        "expected empty stdout on failure, got: {}",
        String::from_utf8_lossy(&output.stdout),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing index blob"),
        "expected error preamble in stderr, got:\n{stderr}",
    );
    assert!(
        stderr.contains(new_hash),
        "expected stderr to name the missing hash, got:\n{stderr}",
    );
    assert!(
        stderr.contains("hello.txt"),
        "expected stderr to name the affected path, got:\n{stderr}",
    );
    assert!(
        stderr.contains("git fetch"),
        "expected stderr to suggest `git fetch`, got:\n{stderr}",
    );
    // Keep the message tight: at most two lines.
    assert!(
        stderr.trim_end_matches('\n').lines().count() <= 2,
        "expected at most two lines, got:\n{stderr}",
    );
}

#[test]
fn succeeds_for_working_tree_diff_with_synthetic_hash() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    let original = "hello\nworld\n";
    let updated = "HELLO\nworld\n";
    let old_hash = init_repo_with_file(path, "hello.txt", original);

    // Mutate the working tree (without committing) and use the synthetic
    // hash that `git diff` would produce: the git-blob hash of the
    // current working-tree content.
    std::fs::write(path.join("hello.txt"), updated).unwrap();
    let new_hash = git(path, &["hash-object", "--stdin"], updated);
    assert_ne!(old_hash, new_hash);

    let diff = format!(
        "diff --git a/hello.txt b/hello.txt\n\
         index {old_hash}..{new_hash} 100644\n\
         --- a/hello.txt\n\
         +++ b/hello.txt\n\
         @@ -1,2 +1,2 @@\n\
         -hello\n\
         +HELLO\n\
          world\n",
    );

    let output = run_deltoids(path, &diff);

    assert!(
        output.status.success(),
        "expected success, got {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello.txt"),
        "expected file header in stdout, got:\n{stdout}",
    );
    assert!(
        stdout.contains("HELLO"),
        "expected updated content in rendered diff, got:\n{stdout}",
    );
}
