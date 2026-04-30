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

use git2::{IndexAddOption, Oid, Repository, Signature};
use tempfile::tempdir;

/// Initialise a git repo at `path` containing a single committed file.
/// Returns the blob hash for the committed file.
fn init_repo_with_file(path: &Path, file_name: &str, content: &str) -> String {
    let repo = Repository::init(path).unwrap();
    std::fs::write(path.join(file_name), content).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all([file_name], IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    tree.get_path(Path::new(file_name))
        .unwrap()
        .id()
        .to_string()
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
    let new_hash = Oid::hash_object(git2::ObjectType::Blob, updated.as_bytes())
        .unwrap()
        .to_string();
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
