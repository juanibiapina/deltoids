//! Integration tests: `deltoids pager` renders files managed by a git
//! content filter (git-crypt, transcrypt, any clean/smudge filter) like
//! any other file, instead of aborting with a "missing index blob" error.
//!
//! Rather than depend on the git-crypt binary, these tests configure a
//! trivial reversible `rot13` clean/smudge filter: the object database
//! holds the "encrypted" (rot13) form while the working tree holds
//! plaintext. That reproduces the same shape as git-crypt — an ODB blob
//! that is not the plaintext — through the general filter mechanism.

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

/// ANSI-strip stdout for text assertions.
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

/// Run `git` in `dir`, asserting success, returning stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
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

/// Init a repo at `dir` with a reversible `rot13` content filter bound to
/// `secret.txt`.
fn init_filtered_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    let rot13 = "tr A-Za-z N-ZA-Mn-za-m";
    git(dir, &["config", "filter.rot13.clean", rot13]);
    git(dir, &["config", "filter.rot13.smudge", rot13]);
    std::fs::write(dir.join(".gitattributes"), "secret.txt filter=rot13\n").unwrap();
}

#[test]
fn unstaged_filtered_change_renders_plaintext() {
    // Commit plaintext v1 (ODB holds rot13 ciphertext), then edit the
    // working tree to plaintext v2 without staging. This is the exact
    // shape of the original git-crypt bug: the old side is an encrypted
    // ODB blob, the new side is the working-tree plaintext.
    let dir = tempdir().unwrap();
    let path = dir.path();
    init_filtered_repo(path);
    std::fs::write(path.join("secret.txt"), "hello\nworld\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);
    std::fs::write(path.join("secret.txt"), "hello\nplanet\n").unwrap();

    // The old side is the committed ciphertext ODB blob; the new side is
    // the plaintext hash of the working tree, matching how git records an
    // unstaged change to a filtered file. Model git's textconv view:
    // plaintext hunks with these index hashes.
    let old_hash = git(path, &["rev-parse", "HEAD:secret.txt"]);
    let old_hash = old_hash.trim();
    // `--no-filters` yields the plaintext hash, matching how git-crypt
    // records an unstaged change (its clean filter is hash-preserving for
    // the working-tree view).
    let new_hash = git(path, &["hash-object", "--no-filters", "secret.txt"]);
    let new_hash = new_hash.trim();
    let diff = format!(
        "diff --git a/secret.txt b/secret.txt\n\
         index {old_hash}..{new_hash} 100644\n\
         --- a/secret.txt\n\
         +++ b/secret.txt\n\
         @@ -1,2 +1,2 @@\n\
         \x20hello\n\
         -world\n\
         +planet\n"
    );

    let output = run_pager_in(path, &diff);
    assert!(output.status.success(), "pager should exit 0");
    let text = stdout_plain(&output);
    assert!(
        text.contains("planet"),
        "expected new plaintext, got:\n{text}"
    );
    // The old side must be decrypted from the ODB blob, not shown as its
    // stored (rot13) form.
    assert!(
        text.contains("world"),
        "expected old plaintext, got:\n{text}"
    );
    assert!(
        !text.contains("jbeyq"),
        "old side must not render as stored ciphertext, got:\n{text}"
    );
    assert!(
        !text.contains("missing index blob"),
        "must not emit missing-blob error, got:\n{text}"
    );
}

#[test]
fn committed_filtered_history_renders_plaintext() {
    // Both sides are committed (ODB) ciphertext blobs, as in `git show`
    // or `git log -p` of a filtered file's history. Resolution must
    // smudge both back to plaintext; reverse-reconstruction from hunks
    // alone could not, since there is no full-file anchor.
    let dir = tempdir().unwrap();
    let path = dir.path();
    init_filtered_repo(path);
    std::fs::write(path.join("secret.txt"), "hello\nworld\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);
    let old_hash = git(path, &["rev-parse", "HEAD:secret.txt"]);
    let old_hash = old_hash.trim().to_string();
    std::fs::write(path.join("secret.txt"), "hello\nplanet\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "change"]);
    let new_hash = git(path, &["rev-parse", "HEAD:secret.txt"]);
    let new_hash = new_hash.trim();
    let diff = format!(
        "diff --git a/secret.txt b/secret.txt\n\
         index {old_hash}..{new_hash} 100644\n\
         --- a/secret.txt\n\
         +++ b/secret.txt\n\
         @@ -1,2 +1,2 @@\n\
         \x20hello\n\
         -world\n\
         +planet\n"
    );

    let output = run_pager_in(path, &diff);
    assert!(output.status.success(), "pager should exit 0");
    let text = stdout_plain(&output);
    assert!(
        text.contains("planet"),
        "expected new plaintext, got:\n{text}"
    );
    assert!(
        text.contains("world"),
        "expected old plaintext, got:\n{text}"
    );
    assert!(
        !text.contains("missing index blob"),
        "must not emit missing-blob error, got:\n{text}"
    );
}
