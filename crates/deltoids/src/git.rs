//! Lightweight wrappers over `git2` used by the [`content`](crate::content)
//! module to resolve before/after content for diff hunks.
//!
//! Available only when the `blob-resolve` cargo feature is enabled.

use std::path::Path;

use git2::{DiffFormat, DiffOptions, ObjectType, Oid, Repository, Status, StatusOptions};

/// A discovered git repository, used to look up blobs by hash.
pub struct Repo(Repository);

/// One change column of a file's `git status --porcelain` XY code:
/// either the staged column (HEAD → index) or the worktree column
/// (index → workdir). `Untracked` only appears in the worktree column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Untracked,
}

/// Per-file staging status mirroring `git status --porcelain` XY codes.
///
/// `staged` is column X (HEAD → index), `unstaged` is column Y (index →
/// workdir). A staged-new-then-edited file has `staged: Some(Added)` and
/// `unstaged: Some(Modified)` (porcelain `AM`); an untracked file has
/// `staged: None` and `unstaged: Some(Untracked)` (porcelain `??`). The
/// `path` is workdir-relative, matching the paths in
/// [`Repo::working_tree_diff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStageStatus {
    pub path: String,
    pub staged: Option<StageChange>,
    pub unstaged: Option<StageChange>,
}

impl Repo {
    /// Discover git repository from current directory.
    pub fn discover() -> Option<Self> {
        Repository::discover(".").ok().map(Repo)
    }

    /// Discover a git repository starting from `path`, walking up to find
    /// the enclosing `.git`. Same as [`Repo::discover`] but anchored at an
    /// explicit path instead of the process working directory.
    pub fn discover_at(path: &Path) -> Option<Self> {
        Repository::discover(path).ok().map(Repo)
    }

    /// The repository's working directory, or `None` for a bare repo.
    /// The review watcher watches this tree for changes.
    pub fn workdir(&self) -> Option<&Path> {
        self.0.workdir()
    }

    /// Whether `path` is gitignored. Accepts an absolute path inside the
    /// working tree or a workdir-relative path.
    ///
    /// Gitignored files never appear in [`Repo::working_tree_diff`], so a
    /// change to one must not trigger a reload. Respects nested
    /// `.gitignore`, `.git/info/exclude`, and global excludes. Returns
    /// `false` on any error (fail open: never miss a real change).
    pub fn is_ignored(&self, path: &Path) -> bool {
        // libgit2 wants a workdir-relative path; map absolute paths down.
        let rel = match self.0.workdir() {
            Some(workdir) => path.strip_prefix(workdir).unwrap_or(path),
            None => path,
        };
        self.0.is_path_ignored(rel).unwrap_or(false)
    }

    /// Full working-tree-equivalent text of the blob named by `hash`, with
    /// git content filters applied (clean/smudge, e.g. git-crypt
    /// decryption), as `git cat-file --filters --path=<path> <hash>`
    /// produces. `path` is a workdir-relative path used to resolve which
    /// filter applies.
    ///
    /// On a non-filtered file this is byte-identical to the raw blob
    /// content, so it is safe to apply to every ODB blob. libgit2 cannot
    /// run external process filters, so this shells out to the git CLI.
    ///
    /// Returns `None` if the hash is null, git is unavailable, the blob is
    /// not in the ODB, the command fails, or the result is not valid
    /// UTF-8. Failing soft lets callers fall through to other resolution
    /// paths.
    pub fn blob_filtered(&self, hash: &str, path: &str) -> Option<String> {
        if is_null_hash(hash) {
            return None;
        }
        let workdir = self.0.workdir()?;
        cat_file_filtered(workdir, hash, path)
    }

    /// Unified diff of the working tree against `HEAD`, as patch text
    /// ready for [`crate::parse::GitDiff::parse`].
    ///
    /// Covers the same set of changes as `git diff HEAD`: staged and
    /// unstaged edits to tracked files, plus untracked files (shown as
    /// additions with their content). Returns an empty string when the
    /// working tree matches `HEAD`. An unborn `HEAD` (a repo with no
    /// commits) is treated as an empty tree, so every file shows as an
    /// addition.
    pub fn working_tree_diff(&self) -> Result<String, String> {
        let head_tree = match self.0.head() {
            Ok(head) => Some(
                head.peel_to_tree()
                    .map_err(|e| format!("failed to read HEAD tree: {e}"))?,
            ),
            // Unborn HEAD (no commits yet): diff against an empty tree.
            Err(_) => None,
        };

        let mut opts = DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);

        let diff = self
            .0
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
            .map_err(|e| format!("failed to diff working tree: {e}"))?;

        let mut out = String::new();
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            // For +/-/context lines git2 strips the origin prefix; the
            // file and hunk headers already carry their full text.
            match line.origin() {
                '+' | '-' | ' ' => out.push(line.origin()),
                _ => {}
            }
            if let Ok(text) = std::str::from_utf8(line.content()) {
                out.push_str(text);
            }
            true
        })
        .map_err(|e| format!("failed to format diff: {e}"))?;

        Ok(out)
    }

    /// Per-file staging status for the working tree, mirroring
    /// `git status --porcelain` XY codes. Each path is workdir-relative,
    /// matching the paths in [`Repo::working_tree_diff`].
    ///
    /// Splits git2's `Status` bitflags into two columns: the `INDEX_*`
    /// flags (HEAD → index) become the `staged` column, the `WT_*` flags
    /// (index → workdir) become the `unstaged` column. Untracked files
    /// report `unstaged: Some(Untracked)` with `staged: None`. Rename
    /// detection is enabled for both columns so a staged/worktree rename
    /// reports `Renamed` rather than a delete + add pair.
    pub fn working_tree_status(&self) -> Result<Vec<FileStageStatus>, String> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);

        let statuses = self
            .0
            .statuses(Some(&mut opts))
            .map_err(|e| format!("failed to read status: {e}"))?;

        let mut out = Vec::new();
        for entry in statuses.iter() {
            let flags = entry.status();
            let staged = staged_change(flags);
            let unstaged = unstaged_change(flags);
            if staged.is_none() && unstaged.is_none() {
                continue;
            }
            // Prefer the post-image path so renames join to the diff by
            // their new name (the diff's `new_path`); fall back to the
            // entry path.
            let Some(path) = entry_new_path(&entry) else {
                continue;
            };
            out.push(FileStageStatus {
                path,
                staged,
                unstaged,
            });
        }
        Ok(out)
    }
}

/// Workdir-relative path of a status entry, preferring the post-image
/// (`new_file`) path from whichever delta exists so a rename reports its
/// new name. Falls back to the entry's own path.
fn entry_new_path(entry: &git2::StatusEntry<'_>) -> Option<String> {
    let from_delta = entry
        .head_to_index()
        .or_else(|| entry.index_to_workdir())
        .and_then(|delta| {
            delta
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned())
        });
    from_delta.or_else(|| entry.path().ok().map(str::to_string))
}

/// Map the `INDEX_*` (staged column) bits of a [`Status`] to a
/// [`StageChange`], or `None` when nothing is staged.
fn staged_change(flags: Status) -> Option<StageChange> {
    if flags.contains(Status::INDEX_NEW) {
        Some(StageChange::Added)
    } else if flags.contains(Status::INDEX_MODIFIED) {
        Some(StageChange::Modified)
    } else if flags.contains(Status::INDEX_DELETED) {
        Some(StageChange::Deleted)
    } else if flags.contains(Status::INDEX_RENAMED) {
        Some(StageChange::Renamed)
    } else if flags.contains(Status::INDEX_TYPECHANGE) {
        Some(StageChange::TypeChanged)
    } else {
        None
    }
}

/// Map the `WT_*` (worktree column) bits of a [`Status`] to a
/// [`StageChange`], or `None` when the worktree matches the index.
/// `WT_NEW` (an untracked file) maps to [`StageChange::Untracked`].
fn unstaged_change(flags: Status) -> Option<StageChange> {
    if flags.contains(Status::WT_NEW) {
        Some(StageChange::Untracked)
    } else if flags.contains(Status::WT_MODIFIED) {
        Some(StageChange::Modified)
    } else if flags.contains(Status::WT_DELETED) {
        Some(StageChange::Deleted)
    } else if flags.contains(Status::WT_RENAMED) {
        Some(StageChange::Renamed)
    } else if flags.contains(Status::WT_TYPECHANGE) {
        Some(StageChange::TypeChanged)
    } else {
        None
    }
}

/// Check if hash represents "no file" (all zeros).
pub fn is_null_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.chars().all(|c| c == '0')
}

/// Check whether `content` hashes to the git blob OID `expected`.
///
/// Accepts full (40-char) and abbreviated hashes. Returns `false` if
/// hashing fails or the hashes don't match.
pub fn blob_hash_matches(content: &str, expected: &str) -> bool {
    let Ok(oid) = Oid::hash_object(ObjectType::Blob, content.as_bytes()) else {
        return false;
    };
    let oid_str = oid.to_string();
    if expected.len() >= oid_str.len() {
        oid_str == *expected
    } else {
        oid_str.starts_with(expected)
    }
}

/// Run `git cat-file --filters --path=<path> <hash>` in `workdir` and
/// return its stdout as UTF-8 text. This is the one external git-CLI
/// shell-out in the library; it stays private so the process dependency
/// (argv, exit status, stderr, decoding) never leaks past
/// [`Repo::blob_filtered`].
///
/// Returns `None` when git cannot be spawned, exits non-zero (blob
/// missing, bad path), or emits non-UTF-8 bytes (a genuinely binary
/// blob).
fn cat_file_filtered(workdir: &Path, hash: &str, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("cat-file")
        .arg("--filters")
        .arg(format!("--path={path}"))
        .arg(hash)
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Init a repo at `dir` with a committable identity configured.
    fn init_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        repo
    }

    /// Stage everything under the working tree and write the index.
    fn stage_all(repo: &Repository) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
    }

    /// Commit the current index as a new commit on `HEAD`.
    fn commit_index(repo: &Repository, msg: &str) {
        let mut index = repo.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap();
    }

    /// Run `git` in `dir` via the CLI, asserting success. Needed for
    /// operations that must run external content filters (clean/smudge),
    /// which libgit2 does not apply.
    fn git_cli(dir: &Path, args: &[&str]) -> std::process::Output {
        let out = std::process::Command::new("git")
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
        out
    }

    /// Set up a repo at `dir` with a reversible `rot13` content filter
    /// bound to `<path>`, then commit `plaintext` at that path via the git
    /// CLI so the ODB holds the cleaned (rot13) form while the working
    /// tree holds `plaintext`. Returns the ODB blob hash of the committed
    /// (cleaned) content. This models a git-crypt-style filter without
    /// needing the git-crypt binary.
    fn commit_filtered(dir: &Path, path: &str, plaintext: &str) -> String {
        git_cli(dir, &["init", "-q"]);
        let rot13 = "tr A-Za-z N-ZA-Mn-za-m";
        git_cli(dir, &["config", "filter.rot13.clean", rot13]);
        git_cli(dir, &["config", "filter.rot13.smudge", rot13]);
        fs::write(dir.join(".gitattributes"), format!("{path} filter=rot13\n")).unwrap();
        fs::write(dir.join(path), plaintext).unwrap();
        git_cli(dir, &["add", "-A"]);
        git_cli(dir, &["commit", "-qm", "init"]);
        let out = git_cli(dir, &["rev-parse", &format!("HEAD:{path}")]);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn working_tree_diff_shows_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("a.txt"), "world\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(patch.contains("a.txt"), "missing path in: {patch}");
        assert!(patch.contains("index "), "missing index header in: {patch}");
        assert!(patch.contains("-hello"), "missing old line in: {patch}");
        assert!(patch.contains("+world"), "missing new line in: {patch}");
    }

    #[test]
    fn working_tree_diff_includes_staged_change() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Change and stage it: vs HEAD this must still appear.
        fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
        stage_all(&repo);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(
            patch.contains("+staged"),
            "staged change missing in: {patch}"
        );
    }

    #[test]
    fn working_tree_diff_includes_untracked_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("new.txt"), "brand new\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(
            patch.contains("new.txt"),
            "untracked path missing in: {patch}"
        );
        assert!(
            patch.contains("+brand new"),
            "untracked content missing in: {patch}"
        );
        assert!(
            patch.contains("/dev/null"),
            "untracked old side should be /dev/null in: {patch}"
        );
    }

    #[test]
    fn working_tree_diff_marks_binary_file_with_index_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        // Commit a binary blob (bytes with a NUL), then change it.
        fs::write(dir.path().join("bin"), b"\x00\x01\x02").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("bin"), b"\x00\x03\x04").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(
            patch.contains("Binary files ") && patch.contains(" differ"),
            "expected a binary delta marker in: {patch}"
        );
        // The index line carries non-null blob hashes.
        let index_line = patch
            .lines()
            .find(|l| l.trim_start().starts_with("index "))
            .expect("index line present");
        assert!(
            !index_line.contains("0000000000000000000000000000000000000000"),
            "expected non-null blob hashes in: {index_line}"
        );
    }

    #[test]
    fn working_tree_diff_empty_when_clean() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        assert_eq!(wrapper.working_tree_diff().unwrap(), "");
    }

    #[test]
    fn working_tree_diff_unborn_head_shows_additions() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "first\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(patch.contains("a.txt"), "missing path in: {patch}");
        assert!(patch.contains("+first"), "missing added line in: {patch}");
    }

    /// Look up one path's stage status from a `working_tree_status` result.
    fn status_of<'a>(statuses: &'a [FileStageStatus], path: &str) -> Option<&'a FileStageStatus> {
        statuses.iter().find(|s| s.path == path)
    }

    #[test]
    fn working_tree_status_staged_add() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("new.txt"), "brand new\n").unwrap();
        stage_all(&repo);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "new.txt").expect("new.txt in status");
        assert_eq!(s.staged, Some(StageChange::Added));
        assert_eq!(s.unstaged, None);
    }

    #[test]
    fn working_tree_status_unstaged_modify() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("a.txt"), "world\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "a.txt").expect("a.txt in status");
        assert_eq!(s.staged, None);
        assert_eq!(s.unstaged, Some(StageChange::Modified));
    }

    #[test]
    fn working_tree_status_staged_then_edited() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Stage a modification, then edit again without staging.
        fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
        stage_all(&repo);
        fs::write(dir.path().join("a.txt"), "staged then edited\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "a.txt").expect("a.txt in status");
        assert_eq!(s.staged, Some(StageChange::Modified));
        assert_eq!(s.unstaged, Some(StageChange::Modified));
    }

    #[test]
    fn working_tree_status_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("new.txt"), "brand new\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "new.txt").expect("new.txt in status");
        assert_eq!(s.staged, None);
        assert_eq!(s.unstaged, Some(StageChange::Untracked));
    }

    #[test]
    fn working_tree_status_staged_delete() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::remove_file(dir.path().join("a.txt")).unwrap();
        stage_all(&repo);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "a.txt").expect("a.txt in status");
        assert_eq!(s.staged, Some(StageChange::Deleted));
        assert_eq!(s.unstaged, None);
    }

    #[test]
    fn working_tree_status_staged_rename() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("old.txt"), "some content here\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Rename by moving the file, then stage both the delete and add
        // so git detects the rename in the index.
        fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        stage_all(&repo);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "new.txt").expect("new.txt in status");
        assert_eq!(s.staged, Some(StageChange::Renamed));
    }

    #[test]
    fn working_tree_status_empty_when_clean() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        assert!(wrapper.working_tree_status().unwrap().is_empty());
    }

    #[test]
    fn is_ignored_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        assert!(wrapper.is_ignored(&dir.path().join("node_modules/x.js")));
        assert!(wrapper.is_ignored(Path::new("node_modules/x.js")));
        assert!(!wrapper.is_ignored(&dir.path().join("src/main.rs")));
        assert!(!wrapper.is_ignored(Path::new("src/main.rs")));
    }

    #[test]
    fn working_tree_diff_round_trips_through_resolve() {
        use crate::content::{self, SideContent};
        use crate::parse::GitDiff;

        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Stage the change so the post-image blob lives in the ODB and
        // resolution does not depend on the process working directory.
        fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        stage_all(&repo);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        let parsed = GitDiff::parse(&patch);
        assert_eq!(parsed.files.len(), 1);

        let resolved = content::retrieve(&parsed.files[0], Some(&wrapper));
        match (resolved.before, resolved.after) {
            (SideContent::Resolved(before), SideContent::Resolved(after)) => {
                assert_eq!(before, "hello\n");
                assert_eq!(after, "world\n");
            }
            _ => panic!("expected both sides resolved"),
        }
    }

    #[test]
    fn working_tree_diff_of_filtered_file_carries_resolvable_hashes() {
        use crate::parse::GitDiff;

        // The TUI feeds libgit2's `working_tree_diff` (not the git CLI)
        // through content resolution. libgit2 cannot run an external clean
        // filter, so for an unstaged change to a filtered file it hashes
        // the working tree as plaintext: old_hash = the committed
        // ("encrypted") ODB blob, new_hash = the plaintext hash. Both
        // sides are then resolvable — old via `blob_filtered`, new via the
        // working-tree read whose plaintext hashes to new_hash.
        let dir = tempfile::tempdir().unwrap();
        let old_hash = commit_filtered(dir.path(), "secret.txt", "hello\nworld\n");
        fs::write(dir.path().join("secret.txt"), "hello\nplanet\n").unwrap();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let patch = wrapper.working_tree_diff().unwrap();
        let parsed = GitDiff::parse(&patch);
        assert_eq!(parsed.files.len(), 1);
        let file = &parsed.files[0];

        // old side resolves to plaintext through the filter. libgit2 may
        // abbreviate the hash; it names the committed ODB blob either way.
        let parsed_old = file.old_hash.as_deref().expect("old_hash present");
        assert!(
            old_hash.starts_with(parsed_old),
            "parsed old_hash {parsed_old} should prefix {old_hash}"
        );
        assert_eq!(
            wrapper.blob_filtered(parsed_old, "secret.txt").as_deref(),
            Some("hello\nworld\n")
        );
        // new side is the plaintext working-tree hash (libgit2 applied no
        // external filter), so the working-tree plaintext verifies against
        // it.
        let new_hash = file.new_hash.as_deref().expect("new_hash present");
        assert!(
            blob_hash_matches("hello\nplanet\n", new_hash),
            "new_hash {new_hash} should match working-tree plaintext"
        );
    }

    #[test]
    fn filtered_diff_resolves_both_sides_through_cat_file() {
        use crate::content::{self, SideContent};
        use crate::parse::GitDiff;

        // Commit plaintext v1 (ODB holds rot13 ciphertext), then commit a
        // modified plaintext v2 (ODB holds ciphertext v2). Both index
        // blobs are "encrypted"; resolution must smudge them back to
        // plaintext. Committing both sides keeps the test independent of
        // the process working directory (no working-tree read).
        let dir = tempfile::tempdir().unwrap();
        let old_hash = commit_filtered(dir.path(), "secret.txt", "hello\nworld\n");
        fs::write(dir.path().join("secret.txt"), "hello\nplanet\n").unwrap();
        git_cli(dir.path(), &["add", "secret.txt"]);
        git_cli(dir.path(), &["commit", "-qm", "change"]);
        let out = git_cli(dir.path(), &["rev-parse", "HEAD:secret.txt"]);
        let new_hash = String::from_utf8(out.stdout).unwrap().trim().to_string();

        // Model git's textconv view: plaintext hunks, ciphertext index
        // hashes.
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
        let parsed = GitDiff::parse(&diff);
        assert_eq!(parsed.files.len(), 1);

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        let resolved = content::retrieve(&parsed.files[0], Some(&wrapper));
        match (resolved.before, resolved.after) {
            (SideContent::Resolved(before), SideContent::Resolved(after)) => {
                assert_eq!(before, "hello\nworld\n");
                assert_eq!(after, "hello\nplanet\n");
            }
            _ => panic!("expected both filtered sides resolved to plaintext"),
        }
    }

    #[test]
    fn null_hash_detection() {
        assert!(is_null_hash("0000000"));
        assert!(is_null_hash("0000000000000000000000000000000000000000"));
        assert!(!is_null_hash("abc1234"));
        assert!(!is_null_hash("000000a"));
        assert!(!is_null_hash(""));
    }

    #[test]
    fn blob_hash_matches_full_and_abbreviated() {
        // git blob hash of "hello\n"
        let content = "hello\n";
        let full = "ce013625030ba8dba906f756967f9e9ca394464a";
        assert!(blob_hash_matches(content, full));
        assert!(blob_hash_matches(content, &full[..7]));
        assert!(blob_hash_matches(content, &full[..11]));
        assert!(!blob_hash_matches(content, "deadbeef"));
        assert!(!blob_hash_matches("other\n", full));
    }

    #[test]
    fn blob_filtered_applies_smudge_filter() {
        let dir = tempfile::tempdir().unwrap();
        let hash = commit_filtered(dir.path(), "secret.txt", "hello world\n");

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        // The ODB blob is the rot13-cleaned ciphertext; blob_filtered runs
        // the smudge filter to recover the working-tree plaintext.
        assert_eq!(
            wrapper.blob_filtered(&hash, "secret.txt").as_deref(),
            Some("hello world\n")
        );
    }

    #[test]
    fn blob_filtered_matches_raw_for_unfiltered_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("plain.txt"), "no filter here\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = head.tree().unwrap();
        let hash = tree.get_name("plain.txt").unwrap().id().to_string();

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        // On a non-filtered file, cat-file --filters is byte-identical to
        // the raw blob content (no clean/smudge transform applied).
        assert_eq!(
            wrapper.blob_filtered(&hash, "plain.txt").as_deref(),
            Some("no filter here\n")
        );
    }

    #[test]
    fn blob_filtered_resolves_abbreviated_hash() {
        // git may reference blobs by an abbreviated hash; blob_filtered
        // must resolve those too.
        let dir = tempfile::tempdir().unwrap();
        let hash = commit_filtered(dir.path(), "secret.txt", "hello world\n");
        let abbrev = &hash[..7];

        let wrapper = Repo(Repository::open(dir.path()).unwrap());
        assert_eq!(
            wrapper.blob_filtered(abbrev, "secret.txt").as_deref(),
            Some("hello world\n")
        );
    }
}
