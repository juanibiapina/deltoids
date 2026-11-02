//! Test repositories driven through the git CLI, with the user's global
//! and system configuration shut out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A git repository at a directory.
pub(crate) struct TestRepo {
    dir: PathBuf,
}

impl TestRepo {
    /// Init a repository at `dir` on branch `main`, with a committer
    /// identity in its local config.
    pub(crate) fn init(dir: &Path) -> Self {
        let repo = Self::open(dir);
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo
    }

    /// The repository at `dir`, which must exist.
    pub(crate) fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// The repository's working directory.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Run `git args` with `input` on stdin and `env` set, asserting
    /// success, and return its stdout.
    pub(crate) fn run(&self, args: &[&str], input: &[u8], env: &[(&str, &str)]) -> Vec<u8> {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// Run `git args`, asserting success, and return its trimmed stdout.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        let out = self.run(args, b"", &[]);
        String::from_utf8(out).unwrap().trim().to_string()
    }

    /// Stage every change under the working tree, deletions included.
    pub(crate) fn stage_all(&self) {
        self.git(&["add", "-A"]);
    }

    /// Commit the index on `HEAD` and return the new commit's id.
    pub(crate) fn commit(&self, message: &str) -> String {
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.rev("HEAD")
    }

    /// Commit the index on `HEAD` as Ada Lovelace at `seconds` since the
    /// epoch, keeping `message` verbatim, and return the commit's id.
    pub(crate) fn commit_at(&self, message: &str, seconds: i64) -> String {
        let args = [
            "commit",
            "-q",
            "--allow-empty",
            "--cleanup=verbatim",
            "-m",
            message,
        ];
        self.run(&args, b"", &ada(&date(seconds)));
        self.rev("HEAD")
    }

    /// Commit `files`, each a root-level path and its text, on top of
    /// `branch` as Ada Lovelace at `seconds`, without touching the index
    /// or the working tree, and move `branch` to the new commit.
    pub(crate) fn commit_files_on(
        &self,
        branch: &str,
        files: &[(&str, &str)],
        message: &str,
        seconds: i64,
    ) -> String {
        let scratch = tempfile::NamedTempFile::new().unwrap();
        let index = scratch.path().to_str().unwrap();
        let with_index = [("GIT_INDEX_FILE", index)];
        self.run(&["read-tree", branch], b"", &with_index);
        for (path, text) in files {
            let info = format!("100644,{},{path}", self.blob(text.as_bytes()));
            self.run(
                &["update-index", "--add", "--cacheinfo", &info],
                b"",
                &with_index,
            );
        }
        let tree = String::from_utf8(self.run(&["write-tree"], b"", &with_index)).unwrap();
        let parent = self.rev(branch);
        let args = ["commit-tree", tree.trim(), "-p", &parent, "-m", message];
        let id = String::from_utf8(self.run(&args, b"", &ada(&date(seconds)))).unwrap();
        let id = id.trim().to_string();
        self.git(&["update-ref", &format!("refs/heads/{branch}"), &id]);
        id
    }

    /// The absolute path of the `.git` directory.
    pub(crate) fn git_dir(&self) -> PathBuf {
        PathBuf::from(self.git(&["rev-parse", "--absolute-git-dir"]))
    }

    /// The full id `spec` resolves to.
    pub(crate) fn rev(&self, spec: &str) -> String {
        self.git(&["rev-parse", spec])
    }

    /// Write `data` to the object database as a blob and return its id.
    pub(crate) fn blob(&self, data: &[u8]) -> String {
        let out = self.run(&["hash-object", "-w", "--stdin"], data, &[]);
        String::from_utf8(out).unwrap().trim().to_string()
    }

    /// The bytes of the object `spec` names, as `git cat-file -p` prints
    /// them.
    pub(crate) fn show(&self, spec: &str) -> Vec<u8> {
        self.run(&["cat-file", "-p", spec], b"", &[])
    }

    /// Point the stage-0 index entry for `path` at blob `id`.
    pub(crate) fn set_index(&self, path: &str, id: &str) {
        let info = format!("100644,{id},{path}");
        self.git(&["update-index", "--cacheinfo", &info]);
    }

    /// Point `HEAD` at branch `name` without touching the index or the
    /// working tree.
    pub(crate) fn set_head(&self, name: &str) {
        self.git(&["symbolic-ref", "HEAD", &format!("refs/heads/{name}")]);
    }
}

/// A git date for `seconds` since the epoch.
fn date(seconds: i64) -> String {
    format!("@{seconds} +0000")
}

/// Author and committer environment for Ada Lovelace at `date`.
fn ada(date: &str) -> [(&'static str, &str); 6] {
    [
        ("GIT_AUTHOR_NAME", "Ada Lovelace"),
        ("GIT_AUTHOR_EMAIL", "ada@example.com"),
        ("GIT_COMMITTER_NAME", "Ada Lovelace"),
        ("GIT_COMMITTER_EMAIL", "ada@example.com"),
        ("GIT_AUTHOR_DATE", date),
        ("GIT_COMMITTER_DATE", date),
    ]
}
