//! Read access to a git repository through `gix`: blobs for the
//! [`content`](crate::content) module, plus the working-tree snapshot,
//! history, and ignore and attribute lookups the TUI uses.
//!
//! Available only when the `blob-resolve` cargo feature is enabled.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use gix::bstr::{BStr, ByteSlice};

/// A discovered git repository.
pub struct Repo {
    repo: gix::Repository,
    git_dir: PathBuf,
    common_dir: PathBuf,
    /// Working-tree texts the last snapshot read, by blob id, so
    /// [`Repo::blob_filtered`] resolves a snapshot's worktree sides from
    /// the bytes its diff used.
    worktree_texts: RefCell<HashMap<gix::ObjectId, String>>,
}

mod diff;
mod history;
mod patch;
mod snapshot;

pub use history::{Branch, BranchBase, Commit};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Patch and staging columns from one working-tree enumeration.
///
/// Reads HEAD and the index once. Each changed working-tree file is read
/// once, and those bytes feed both its hash and its hunks. No index
/// changes are written by this read.
#[derive(Debug, PartialEq, Eq)]
pub struct WorkingTreeSnapshot {
    /// HEAD → worktree for every changed file.
    pub patch: String,
    /// HEAD → index, limited to files with both staged and unstaged changes.
    pub staged_patch: String,
    /// Index → worktree, limited to files with both staged and unstaged changes.
    pub unstaged_patch: String,
    pub stages: Vec<FileStageStatus>,
}

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
    /// Paths through HEAD, index and worktree, ending at the displayed worktree path.
    /// Includes rename companions needed for whole-file actions.
    pub paths: Vec<String>,
    pub staged: Option<StageChange>,
    pub unstaged: Option<StageChange>,
}

impl Repo {
    /// Discover git repository from current directory.
    pub fn discover() -> Option<Self> {
        Self::discover_at(&std::env::current_dir().ok()?)
    }

    /// Discover a git repository starting from `path`, walking up to find
    /// the enclosing `.git`. Same as [`Repo::discover`] but anchored at an
    /// explicit path instead of the process working directory.
    ///
    /// The path is canonicalized first, so [`Self::workdir`],
    /// [`Self::git_dir`], and [`Self::common_dir`] are absolute and free
    /// of symlinks and `..`, matching the paths filesystem watchers
    /// report.
    pub fn discover_at(path: &Path) -> Option<Self> {
        let path = std::fs::canonicalize(path).ok()?;
        let repo = gix::discover(path).ok()?;
        let canonical = |dir: &Path| std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
        Some(Repo {
            git_dir: canonical(repo.git_dir()),
            common_dir: canonical(repo.common_dir()),
            repo,
            worktree_texts: RefCell::default(),
        })
    }

    /// The repository's working directory, or `None` for a bare repo.
    /// The review watcher watches this tree for changes.
    pub fn workdir(&self) -> Option<&Path> {
        self.repo.workdir()
    }

    /// Directory containing this worktree's HEAD and index.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Directory containing shared refs and repository configuration.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// The value of `name` in the repository's own config, ignoring global
    /// and system config. Reads the file on every call, so a value another
    /// process wrote since [`Self::discover_at`] shows up.
    pub fn local_config_string(&self, name: &str) -> Option<String> {
        let path = self.common_dir.join("config");
        let file =
            gix::config::File::from_path_no_includes(path, gix::config::Source::Local).ok()?;
        file.string(name)
            .map(|value| value.to_str_lossy().into_owned())
    }

    /// A fingerprint of `HEAD` and every ref's name and target. Two equal
    /// fingerprints mean no ref moved, appeared, or disappeared between
    /// the reads.
    pub fn refs_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut hash_ref = |reference: &gix::Reference<'_>| {
            reference.name().as_bstr().hash(&mut hasher);
            match reference.target() {
                gix::refs::TargetRef::Object(id) => (0u8, id.as_bytes()).hash(&mut hasher),
                gix::refs::TargetRef::Symbolic(name) => (1u8, name.as_bstr()).hash(&mut hasher),
            }
        };
        if let Ok(head) = self.repo.find_reference("HEAD") {
            hash_ref(&head);
        }
        if let Ok(references) = self.repo.references()
            && let Ok(all) = references.all()
        {
            for reference in all.flatten() {
                hash_ref(&reference);
            }
        }
        hasher.finish()
    }

    /// Whether `path` is gitignored, as [`Ignores::is_ignored`] decides.
    /// Loads the index and exclude files on every call; use
    /// [`Repo::ignores`] to check many paths.
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.ignores()
            .is_some_and(|mut ignores| ignores.is_ignored(path))
    }

    /// The ignore rules and index as they are now, for checking many
    /// paths. `None` for a bare repository or when either fails to load.
    pub fn ignores(&self) -> Option<Ignores<'_>> {
        let workdir = self.repo.workdir()?;
        let index = self.repo.index_or_empty().ok()?;
        let excludes = self
            .repo
            .excludes(
                &index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            )
            .ok()?;
        Some(Ignores {
            workdir,
            index,
            excludes,
        })
    }

    /// Whether `path` (workdir-relative) names a content filter driver
    /// through the `filter` attribute, as git-crypt and transcrypt set.
    ///
    /// A diff compares the stored (cleaned) bytes of such files, so its
    /// binary verdict can disagree with what git shows through the
    /// filter. Returns `false` when the attribute is unset, unspecified,
    /// or the lookup fails.
    pub fn has_content_filter(&self, path: &str) -> bool {
        let Ok(index) = self.repo.index_or_empty() else {
            return false;
        };
        let Ok(mut attributes) = self.repo.attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        ) else {
            return false;
        };
        let mut outcome = attributes.selected_attribute_matches(["filter"]);
        let Ok(platform) = attributes.at_path(path, Some(gix::index::entry::Mode::FILE)) else {
            return false;
        };
        platform.matching_attributes(&mut outcome);
        outcome
            .iter_selected()
            .any(|m| matches!(m.assignment.state, gix::attrs::StateRef::Value(_)))
    }

    /// Full working-tree-equivalent text of the blob named by `hash`, with
    /// git's checkout conversions applied (filter drivers such as
    /// git-crypt, `ident`, line endings, `working-tree-encoding`), as
    /// `git cat-file --filters --path=<path> <hash>` produces. `path` is a
    /// workdir-relative path used to resolve which conversions apply.
    ///
    /// A working-tree text the last [`Self::working_tree_snapshot`] read
    /// resolves as it was read, without conversions.
    ///
    /// Returns `None` if the hash is null, the blob is not in the ODB, a
    /// filter fails, or the result is not valid UTF-8. Failing soft lets
    /// callers fall through to other resolution paths.
    pub fn blob_filtered(&self, hash: &str, path: &str) -> Option<String> {
        if is_null_hash(hash) {
            return None;
        }
        if let Ok(id) = gix::ObjectId::from_hex(hash.as_bytes())
            && let Some(text) = self.worktree_texts.borrow().get(&id)
        {
            return Some(text.clone());
        }
        let id = self.repo.rev_parse_single(hash).ok()?.detach();
        let blob = self.repo.find_blob(id).ok()?;
        let (mut pipeline, _) = self.repo.filter_pipeline(None).ok()?;
        let mut converted = pipeline
            .convert_to_worktree(&blob.data, path.into(), Default::default())
            .ok()?;
        let mut out = Vec::new();
        converted.read_to_end(&mut out).ok()?;
        String::from_utf8(out).ok()
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
    ///
    /// A file whose type changed (regular ↔ symlink ↔ submodule) is
    /// reported as a single typechange entry with old and new modes.
    pub fn working_tree_diff(&self) -> Result<String, String> {
        self.read_snapshot()
            .map(|snapshot| snapshot.patch)
            .map_err(|e| format!("failed to compute working-tree diff: {e}"))
    }

    /// Read the net patch and both staging columns with one workdir scan.
    ///
    /// A `HEAD` that fails to resolve (other than an unborn branch) is an
    /// error, so callers can retain the last complete view.
    pub fn working_tree_snapshot(&self) -> Result<WorkingTreeSnapshot, String> {
        self.read_snapshot()
            .map_err(|e| format!("failed to read working-tree snapshot: {e}"))
    }

    /// Read a snapshot and keep its working-tree texts for
    /// [`Self::blob_filtered`].
    fn read_snapshot(&self) -> Result<WorkingTreeSnapshot, Error> {
        let (snapshot, overlay) = snapshot::read(&self.repo)?;
        let texts = overlay
            .into_iter()
            .filter_map(|(id, data)| Some((id, String::from_utf8(data).ok()?)))
            .collect();
        *self.worktree_texts.borrow_mut() = texts;
        Ok(snapshot)
    }

    /// Per-file staging status for the working tree, mirroring
    /// `git status --porcelain` XY codes. Each path is workdir-relative,
    /// matching the paths in [`Repo::working_tree_diff`]. Rename
    /// detection runs in both columns, and untracked files can be rename
    /// targets, so a moved file reports `Renamed` rather than a delete
    /// and an add.
    pub fn working_tree_status(&self) -> Result<Vec<FileStageStatus>, String> {
        snapshot::stages(&self.repo).map_err(|e| format!("failed to read status: {e}"))
    }
}

/// A snapshot of a repository's ignore rules and index from
/// [`Repo::ignores`]. Edits to either after the snapshot are not seen.
pub struct Ignores<'repo> {
    workdir: &'repo Path,
    index: gix::worktree::Index,
    excludes: gix::AttributeStack<'repo>,
}

impl Ignores<'_> {
    /// Whether `path` is gitignored. Accepts an absolute path inside the
    /// working tree or a workdir-relative path.
    ///
    /// Gitignored files never appear in [`Repo::working_tree_diff`], so a
    /// change to one must not trigger a reload. A path is ignored when it
    /// or a directory above it matches `.gitignore`,
    /// `.git/info/exclude`, or the global excludes, and the index tracks
    /// nothing at it: no file at a file path, and no file below a
    /// directory path. Returns `false` on any error (fail open: never
    /// miss a real change).
    pub fn is_ignored(&mut self, path: &Path) -> bool {
        let Some(rel) = relative_to(self.workdir, path) else {
            return false;
        };
        let is_dir = self.workdir.join(&rel).is_dir();
        !self.is_tracked(&rel, is_dir) && self.is_excluded(&rel, is_dir)
    }

    /// Whether `rel` or a directory above it matches an exclude pattern.
    fn is_excluded(&mut self, rel: &Path, is_dir: bool) -> bool {
        let mut ancestors: Vec<&Path> = rel
            .ancestors()
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        ancestors.reverse();
        let leaf = ancestors.len().saturating_sub(1);
        ancestors.iter().enumerate().any(|(depth, ancestor)| {
            let mode = if depth < leaf || is_dir {
                gix::index::entry::Mode::DIR
            } else {
                gix::index::entry::Mode::FILE
            };
            self.excludes
                .at_path(ancestor, Some(mode))
                .is_ok_and(|platform| platform.is_excluded())
        })
    }

    /// Whether the index has the file `rel`, or a file below the
    /// directory `rel`.
    fn is_tracked(&self, rel: &Path, is_dir: bool) -> bool {
        let Ok(path) = gix::path::into_bstr(rel) else {
            return false;
        };
        let path = gix::path::to_unix_separators_on_windows(path);
        if is_dir {
            self.index.path_is_directory(path.as_ref())
        } else {
            self.index.entry_by_path(path.as_ref()).is_some()
        }
    }
}

/// `path` relative to `workdir`. An absolute `path` may name the tree
/// through a symlink (`/var` for `/private/var` on macOS), so its deepest
/// existing ancestor is resolved first. `None` outside the tree.
fn relative_to(workdir: &Path, path: &Path) -> Option<std::path::PathBuf> {
    if !path.is_absolute() {
        return Some(path.to_owned());
    }
    if let Ok(rel) = path.strip_prefix(workdir) {
        return Some(rel.to_owned());
    }
    let ancestor = path.ancestors().find(|a| a.exists())?;
    let real = ancestor
        .canonicalize()
        .ok()?
        .join(path.strip_prefix(ancestor).ok()?);
    real.strip_prefix(workdir).ok().map(Path::to_owned)
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
    let Ok(oid) = blob_id(content.as_bytes()) else {
        return false;
    };
    let oid_str = oid.to_string();
    if expected.len() >= oid_str.len() {
        oid_str == *expected
    } else {
        oid_str.starts_with(expected)
    }
}

/// The git blob id of `data`, as `git hash-object` computes it without
/// filters.
fn blob_id(data: &[u8]) -> Result<gix::ObjectId, Error> {
    Ok(gix::objs::compute_hash(
        gix::hash::Kind::Sha1,
        gix::objs::Kind::Blob,
        data,
    )?)
}

fn path_str(path: &BStr) -> String {
    path.to_str_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    mod history;
    mod snapshot;

    use super::*;
    use std::fs;
    use std::path::Path;

    /// A test repository driven through the git CLI, with the user's
    /// global and system configuration shut out.
    struct Fixture {
        dir: std::path::PathBuf,
    }

    impl Fixture {
        /// Run `git args` with `input` on stdin, asserting success, and
        /// return its trimmed stdout.
        fn git_with(&self, args: &[&str], input: &[u8], env: &[(&str, &str)]) -> String {
            use std::io::Write;
            let mut child = std::process::Command::new("git")
                .args(args)
                .current_dir(&self.dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .envs(env.iter().copied())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        }

        fn git(&self, args: &[&str]) -> String {
            self.git_with(args, b"", &[])
        }

        /// The full id `spec` resolves to.
        fn rev(&self, spec: &str) -> String {
            self.git(&["rev-parse", spec])
        }

        /// The checked-out branch's short name.
        fn branch(&self) -> String {
            self.git(&["symbolic-ref", "--short", "HEAD"])
        }
    }

    /// Init a repo at `dir`.
    fn init_repo(dir: &Path) -> Fixture {
        let repo = Fixture {
            dir: dir.to_path_buf(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// Stage every change under the working tree, deletions included.
    fn stage_all(repo: &Fixture) {
        repo.git(&["add", "-A"]);
    }

    /// Commit the current index as a new commit on `HEAD`.
    fn commit_index(repo: &Fixture, msg: &str) {
        repo.git(&["commit", "-q", "--allow-empty", "-m", msg]);
    }

    /// Open the [`Repo`] at `dir`.
    fn open(dir: &Path) -> Repo {
        Repo::discover_at(dir).unwrap()
    }

    /// Run `git args` in `dir`, asserting success.
    fn git_cli(dir: &Path, args: &[&str]) -> String {
        Fixture {
            dir: dir.to_path_buf(),
        }
        .git(args)
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
        git_cli(dir, &["rev-parse", &format!("HEAD:{path}")])
    }

    #[test]
    fn working_tree_diff_shows_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        fs::write(dir.path().join("a.txt"), "world\n").unwrap();

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(patch.contains("a.txt"), "missing path in: {patch}");
        assert!(patch.contains("index "), "missing index header in: {patch}");
        assert!(patch.contains("-hello"), "missing old line in: {patch}");
        assert!(patch.contains("+world"), "missing new line in: {patch}");
    }

    #[test]
    fn working_tree_snapshot_survives_files_rewritten_in_place() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // A writer thread truncates and rewrites one file with a varying
        // length, as an editor saving in place does, while the main thread
        // reads snapshots. Every read must succeed: the scan reads each
        // file into memory, so a write mid-read can only change the bytes
        // it sees. (libgit2 memory-mapped the file and died of SIGBUS when
        // a truncate landed mid-read.)
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let path = dir.path().join("a.txt");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_writer = stop.clone();
        let writer = std::thread::spawn(move || {
            let mut n = 0usize;
            while !stop_writer.load(Ordering::Relaxed) {
                let body = "x".repeat(n % 64 * 1024);
                fs::write(&path, format!("{body}\n")).unwrap();
                n += 1;
            }
        });

        let wrapper = open(dir.path());
        for _ in 0..50 {
            if let Err(error) = wrapper.working_tree_snapshot() {
                panic!("a write in place must not fail the snapshot: {error}");
            }
        }

        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
        assert_eq!(wrapper.working_tree_diff().unwrap(), "");
    }

    #[test]
    fn working_tree_diff_unborn_head_shows_additions() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "first\n").unwrap();

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(patch.contains("a.txt"), "missing path in: {patch}");
        assert!(patch.contains("+first"), "missing added line in: {patch}");
    }

    #[test]
    fn working_tree_diff_detects_staged_rename() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(
            dir.path().join("old.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Pure rename: move the file and stage the delete + add.
        fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        stage_all(&repo);

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(
            patch.contains("rename from old.txt"),
            "missing rename from in: {patch}"
        );
        assert!(
            patch.contains("rename to new.txt"),
            "missing rename to in: {patch}"
        );
    }

    #[test]
    fn working_tree_diff_detects_content_changed_rename() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(
            dir.path().join("old.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Rename plus a small content edit: still one rename delta, but
        // with an index line and hunks.
        fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        fs::write(
            dir.path().join("new.txt"),
            "line one\nline two CHANGED\nline three\nline four\nline five\n",
        )
        .unwrap();
        stage_all(&repo);

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();
        assert!(
            patch.contains("rename from old.txt"),
            "missing rename from in: {patch}"
        );
        assert!(
            patch.contains("rename to new.txt"),
            "missing rename to in: {patch}"
        );
        assert!(patch.contains("index "), "missing index header in: {patch}");
        assert!(
            patch.contains("+line two CHANGED"),
            "missing changed line in: {patch}"
        );
    }

    #[test]
    fn working_tree_diff_type_change_is_single_delta() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
        fs::write(dir.path().join("target.txt"), "target content\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Replace the regular file with a symlink and stage it.
        fs::remove_file(dir.path().join("f.txt")).unwrap();
        std::os::unix::fs::symlink("target.txt", dir.path().join("f.txt")).unwrap();
        stage_all(&repo);

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();

        // One diff block for f.txt (a single typechange delta), not a
        // delete + add pair.
        let blocks = patch.matches("diff --git a/f.txt b/f.txt").count();
        assert_eq!(blocks, 1, "expected one f.txt block in: {patch}");
        assert!(
            patch.contains("old mode 100644"),
            "missing old mode in: {patch}"
        );
        assert!(
            patch.contains("new mode 120000"),
            "missing new mode in: {patch}"
        );
        assert!(
            !patch.contains("deleted file mode") && !patch.contains("new file mode"),
            "type change must not split into delete + add in: {patch}"
        );
    }

    /// Build an outer repo at `dir` with a submodule `sub` pinned at an
    /// older commit, then bump the working-tree checkout to a newer commit
    /// so the outer tree shows an unstaged submodule commit bump.
    fn setup_submodule_bump(dir: &Path) {
        let mut src_name = dir.file_name().unwrap().to_os_string();
        src_name.push("_sub_src");
        let src = dir.with_file_name(src_name);
        fs::create_dir_all(&src).unwrap();
        git_cli(&src, &["init", "-q"]);
        fs::write(src.join("file.txt"), "v1\n").unwrap();
        git_cli(&src, &["add", "-A"]);
        git_cli(&src, &["commit", "-qm", "sub v1"]);
        let v1 = git_cli(&src, &["rev-parse", "HEAD"]);
        fs::write(src.join("file.txt"), "v2\n").unwrap();
        git_cli(&src, &["add", "-A"]);
        git_cli(&src, &["commit", "-qm", "sub v2"]);

        git_cli(dir, &["init", "-q"]);
        fs::write(dir.join("readme.txt"), "hello\n").unwrap();
        git_cli(dir, &["add", "-A"]);
        git_cli(dir, &["commit", "-qm", "init"]);
        git_cli(
            dir,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                src.to_str().unwrap(),
                "sub",
            ],
        );
        git_cli(&dir.join("sub"), &["checkout", "-q", &v1]);
        git_cli(dir, &["add", "sub"]);
        git_cli(dir, &["commit", "-qm", "add submodule at v1"]);
        git_cli(&dir.join("sub"), &["checkout", "-q", "-"]);
    }

    #[test]
    fn working_tree_diff_submodule_bump_carries_gitlink_mode() {
        let dir = tempfile::tempdir().unwrap();
        setup_submodule_bump(dir.path());

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();

        // The submodule delta carries the `160000` gitlink mode on the
        // index line, and its "content" is a `Subproject commit` line, not
        // a resolvable blob.
        assert!(
            patch.contains("diff --git a/sub b/sub"),
            "missing submodule delta in: {patch}"
        );
        assert!(patch.contains("160000"), "missing gitlink mode in: {patch}");
        assert!(
            patch.contains("Subproject commit"),
            "missing subproject marker in: {patch}"
        );

        // The status side reports the submodule path with an unstaged
        // change (the working-tree checkout moved off the recorded commit).
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "sub").expect("sub in status");
        assert!(
            s.unstaged.is_some(),
            "expected an unstaged change on the submodule, got {s:?}"
        );
    }

    #[test]
    fn working_tree_diff_symlink_to_file_is_single_delta() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join("target.txt"), "target content\n").unwrap();
        std::os::unix::fs::symlink("target.txt", dir.path().join("f.txt")).unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Replace the symlink with a regular file and stage it.
        fs::remove_file(dir.path().join("f.txt")).unwrap();
        fs::write(dir.path().join("f.txt"), "now regular\n").unwrap();
        stage_all(&repo);

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();

        let blocks = patch.matches("diff --git a/f.txt b/f.txt").count();
        assert_eq!(blocks, 1, "expected one f.txt block in: {patch}");
        assert!(
            patch.contains("old mode 120000"),
            "missing old mode in: {patch}"
        );
        assert!(
            patch.contains("new mode 100644"),
            "missing new mode in: {patch}"
        );
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
        let statuses = wrapper.working_tree_status().unwrap();
        let s = status_of(&statuses, "new.txt").expect("new.txt in status");
        assert_eq!(s.staged, Some(StageChange::Renamed));
    }

    #[test]
    fn working_tree_status_content_changed_rename() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(
            dir.path().join("old.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Rename plus an edit: git reports this as a rename (R), not a
        // plain modify, even though the content also changed.
        fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        fs::write(
            dir.path().join("new.txt"),
            "line one\nline two CHANGED\nline three\nline four\nline five\n",
        )
        .unwrap();
        stage_all(&repo);

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
        assert!(wrapper.working_tree_status().unwrap().is_empty());
    }

    #[test]
    fn is_ignored_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = open(dir.path());
        assert!(wrapper.is_ignored(&dir.path().join("node_modules/x.js")));
        assert!(wrapper.is_ignored(Path::new("node_modules/x.js")));
        assert!(!wrapper.is_ignored(&dir.path().join("src/main.rs")));
        assert!(!wrapper.is_ignored(Path::new("src/main.rs")));
    }

    #[test]
    fn tracked_paths_are_never_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "vendor/\n*.log\n").unwrap();
        fs::create_dir(dir.path().join("vendor")).unwrap();
        fs::write(dir.path().join("vendor/lib.rs"), "lib").unwrap();
        fs::write(dir.path().join("vendor/extra.rs"), "extra").unwrap();
        fs::write(dir.path().join("kept.log"), "log").unwrap();
        fs::write(dir.path().join("other.log"), "log").unwrap();
        repo.git(&["add", "-f", ".gitignore", "vendor/lib.rs", "kept.log"]);
        commit_index(&repo, "init");

        let wrapper = open(dir.path());
        assert!(!wrapper.is_ignored(&dir.path().join("vendor/lib.rs")));
        assert!(!wrapper.is_ignored(&dir.path().join("vendor")));
        assert!(!wrapper.is_ignored(&dir.path().join("kept.log")));
        assert!(wrapper.is_ignored(&dir.path().join("vendor/extra.rs")));
        assert!(wrapper.is_ignored(&dir.path().join("other.log")));
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

        let wrapper = open(dir.path());
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

        // The TUI feeds `working_tree_diff` (not the git CLI) through
        // content resolution. For an unstaged change to a filtered file the
        // working tree is hashed as read, without filters: old_hash = the
        // committed ("encrypted") ODB blob, new_hash = the plaintext hash.
        // Both sides are then resolvable — old via `blob_filtered`, new via
        // the working-tree read whose plaintext hashes to new_hash.
        let dir = tempfile::tempdir().unwrap();
        let old_hash = commit_filtered(dir.path(), "secret.txt", "hello\nworld\n");
        fs::write(dir.path().join("secret.txt"), "hello\nplanet\n").unwrap();

        let wrapper = open(dir.path());
        let patch = wrapper.working_tree_diff().unwrap();
        let parsed = GitDiff::parse(&patch);
        assert_eq!(parsed.files.len(), 1);
        let file = &parsed.files[0];

        // old side resolves to plaintext through the filter.
        let parsed_old = file.old_hash.as_deref().expect("old_hash present");
        assert!(
            old_hash.starts_with(parsed_old),
            "parsed old_hash {parsed_old} should prefix {old_hash}"
        );
        assert_eq!(
            wrapper.blob_filtered(parsed_old, "secret.txt").as_deref(),
            Some("hello\nworld\n")
        );
        // new side is the plaintext working-tree hash, so the
        // working-tree plaintext verifies against it.
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
        let new_hash = git_cli(dir.path(), &["rev-parse", "HEAD:secret.txt"]);

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

        let wrapper = open(dir.path());
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

        let wrapper = open(dir.path());
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

        let hash = repo.rev("HEAD:plain.txt");

        let wrapper = open(dir.path());
        // On a non-filtered file, cat-file --filters is byte-identical to
        // the raw blob content (no clean/smudge transform applied).
        assert_eq!(
            wrapper.blob_filtered(&hash, "plain.txt").as_deref(),
            Some("no filter here\n")
        );
    }

    #[test]
    fn blob_filtered_converts_line_endings_like_a_checkout() {
        let dir = tempfile::tempdir().unwrap();
        git_cli(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join(".gitattributes"), "crlf.txt eol=crlf\n").unwrap();
        fs::write(dir.path().join("crlf.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.path().join("auto.txt"), "one\ntwo\n").unwrap();
        git_cli(dir.path(), &["add", "-A"]);
        git_cli(dir.path(), &["commit", "-qm", "init"]);
        let hash = |path: &str| git_cli(dir.path(), &["rev-parse", &format!("HEAD:{path}")]);
        let (crlf, auto) = (hash("crlf.txt"), hash("auto.txt"));

        let wrapper = open(dir.path());
        assert_eq!(
            wrapper.blob_filtered(&crlf, "crlf.txt").as_deref(),
            Some("one\r\ntwo\r\n")
        );
        assert_eq!(
            wrapper.blob_filtered(&auto, "auto.txt").as_deref(),
            Some("one\ntwo\n")
        );
        git_cli(dir.path(), &["config", "core.autocrlf", "true"]);
        let wrapper = open(dir.path());
        assert_eq!(
            wrapper.blob_filtered(&auto, "auto.txt").as_deref(),
            Some("one\r\ntwo\r\n")
        );
    }

    #[test]
    fn blob_filtered_resolves_abbreviated_hash() {
        // git may reference blobs by an abbreviated hash; blob_filtered
        // must resolve those too.
        let dir = tempfile::tempdir().unwrap();
        let hash = commit_filtered(dir.path(), "secret.txt", "hello world\n");
        let abbrev = &hash[..7];

        let wrapper = open(dir.path());
        assert_eq!(
            wrapper.blob_filtered(abbrev, "secret.txt").as_deref(),
            Some("hello world\n")
        );
    }
}
