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

    /// Read a blob's text content by hash (abbreviated or full).
    /// Returns None if the hash is null, the blob is missing, or its
    /// bytes are not valid UTF-8.
    pub fn blob_text(&self, hash: &str) -> Option<String> {
        if is_null_hash(hash) {
            return None;
        }

        // For full 40-char hashes, parse directly; for abbreviated, use revparse
        let oid = if hash.len() == 40 {
            Oid::from_str(hash).ok()
        } else {
            self.0.revparse_single(hash).ok().map(|obj| obj.id())
        }?;

        let blob = self.0.find_blob(oid).ok()?;
        std::str::from_utf8(blob.content()).ok().map(String::from)
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
    fn blob_lookup_abbreviated_hash() {
        // This test requires running in a git repo
        let repo = match Repo::discover() {
            Some(r) => r,
            None => return, // Skip if not in a git repo
        };

        // Get HEAD commit's tree to find a known blob (file, not directory)
        let head = repo.0.head().unwrap().peel_to_commit().unwrap();
        let tree = head.tree().unwrap();
        let entry = tree
            .iter()
            .find(|e| e.kind() == Some(git2::ObjectType::Blob))
            .expect("should have at least one blob in tree");
        let full_hash = entry.id().to_string();
        let abbrev_hash = &full_hash[..7];

        // Both should resolve to the same content
        let full_content = repo.blob_text(&full_hash);
        let abbrev_content = repo.blob_text(abbrev_hash);

        assert!(full_content.is_some(), "full hash should resolve");
        assert!(abbrev_content.is_some(), "abbreviated hash should resolve");
        assert_eq!(
            full_content, abbrev_content,
            "both should return same content"
        );
    }
}
