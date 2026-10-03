//! Data axis for `review`: parse a diff, resolve before/after blob
//! content against the repo, and compute per-file [`Diff`]s. The owned
//! [`Model`] is rebuilt wholesale on each working-tree reload.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::{Diff, LineKind, SymlinkView, content, git};

use crate::sidebar::{ChangeKind, StageStatus, display_path};

/// Describes whether and how the diff can be refreshed mid-session.
pub(super) enum DiffSource<'a> {
    /// Piped stdin: a closed stream, never refreshes.
    Static,
    /// Bare repo: re-diff the working tree when files change on disk.
    WorkingTree(&'a git::Repo),
}

/// The owned data the TUI renders: resolved files plus their per-file
/// bodies. Rebuilt wholesale on each working-tree reload.
pub(super) struct Model {
    pub(super) files: Vec<ResolvedFile>,
    pub(super) bodies: Vec<FileBody>,
    /// Per-file two-column staging status, keyed by workdir-relative
    /// path (matching `display_path`). Empty for piped diffs / no repo,
    /// in which case the sidebar falls back to single-letter status.
    pub(super) stages: HashMap<String, StageStatus>,
}

/// Per-file rendered representation, decided once at build time so the
/// diff pane and the sidebar always agree: a computed text diff, or a
/// symlink change view (which bypasses `Diff::compute` and content
/// resolution entirely).
#[derive(Clone)]
pub(super) enum FileBody {
    Diff(Arc<Diff>),
    Symlink(SymlinkView),
    /// A binary change: no textual diff. Decided from the parsed diff, so
    /// content resolution never touches the ODB or the working tree.
    Binary,
    /// Index and worktree changes whose net HEAD diff is empty.
    StatusOnly,
    /// A submodule (gitlink, git mode `160000`) change: its "content" is
    /// a commit OID, not a blob, so content resolution would fail and
    /// blank the panel. Decided from the parsed diff; the diff pane
    /// renders a placeholder from the old/new commit hashes.
    Submodule {
        old_commit: Option<String>,
        new_commit: Option<String>,
    },
}

/// Parse `input`, resolve every file's before/after content against
/// `repo`, and compute per-file [`Diff`]s.
#[cfg(test)]
pub(super) fn build_model(
    input: &str,
    repo: Option<&git::Repo>,
    stages: HashMap<String, StageStatus>,
) -> Result<Model, String> {
    finish_model(GitDiff::parse(input), repo, stages)
}

pub(super) fn build_working_model(
    snapshot: &git::WorkingTreeSnapshot,
    repo: &git::Repo,
) -> Result<Model, String> {
    let mut parsed = GitDiff::parse(&snapshot.patch);
    let mut deleted_paths: HashMap<_, _> = parsed
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.new_path == "/dev/null")
        .map(|(index, file)| (file.old_path.clone(), index))
        .collect();
    let mut added_paths: HashMap<_, _> = parsed
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.old_path == "/dev/null")
        .map(|(index, file)| (file.new_path.clone(), index))
        .collect();
    let mut removed = HashSet::new();
    // A chained rename can arrive as a net deletion/addition pair. Its status
    // record retains the HEAD/index/worktree relationship needed to join it.
    for status in &snapshot.stages {
        if status.staged != Some(git::StageChange::Renamed)
            && status.unstaged != Some(git::StageChange::Renamed)
        {
            continue;
        }
        let (Some(old), Some(new)) = (status.paths.first(), status.paths.last()) else {
            continue;
        };
        if old == new {
            continue;
        }
        if let (Some(deleted), Some(added)) = (deleted_paths.remove(old), added_paths.remove(new)) {
            let old_hash = parsed.files[deleted].old_hash.clone();
            let old_mode = parsed.files[deleted].old_mode.clone();
            let after = &mut parsed.files[added];
            after.old_path = old.clone();
            after.rename_from = Some(old.clone());
            after.old_hash = old_hash;
            after.old_mode = old_mode;
            after
                .preamble
                .retain(|line| !line.starts_with("new file mode "));
            removed.insert(deleted);
        }
    }
    let mut index = 0;
    parsed.files.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    finish_model(parsed, Some(repo), stage_map(&snapshot.stages))
}

fn finish_model(
    parsed: GitDiff,
    repo: Option<&git::Repo>,
    stages: HashMap<String, StageStatus>,
) -> Result<Model, String> {
    let mut files = resolve(parsed, repo)?;
    let mut bodies = precompute_bodies(&files);
    let present: HashSet<_> = files.iter().map(|file| display_path(&file.file)).collect();
    let mut missing: Vec<_> = stages
        .keys()
        .filter(|path| !present.contains(path.as_str()))
        .cloned()
        .collect();
    missing.sort();
    for path in missing {
        files.push(ResolvedFile {
            file: FileDiff {
                preamble: Vec::new(),
                old_path: path.clone(),
                new_path: path,
                rename_from: None,
                old_hash: None,
                new_hash: None,
                old_mode: None,
                new_mode: None,
                hunks: Vec::new(),
            },
            before: String::new(),
            after: String::new(),
        });
        bodies.push(FileBody::StatusOnly);
    }
    Ok(Model {
        files,
        bodies,
        stages,
    })
}

/// Index supplied staging records by path for the sidebar join.
pub(super) fn stage_map(
    statuses: impl AsRef<[git::FileStageStatus]>,
) -> HashMap<String, StageStatus> {
    statuses
        .as_ref()
        .iter()
        .map(|s| {
            (
                s.paths.last().unwrap_or(&s.path).clone(),
                StageStatus {
                    staged: s.staged.map(map_change),
                    unstaged: s.unstaged.map(map_change),
                },
            )
        })
        .collect()
}

/// Map a `deltoids::git::StageChange` to the sidebar's [`ChangeKind`].
fn map_change(change: git::StageChange) -> ChangeKind {
    match change {
        git::StageChange::Added => ChangeKind::Added,
        git::StageChange::Modified => ChangeKind::Modified,
        git::StageChange::Deleted => ChangeKind::Deleted,
        git::StageChange::Renamed => ChangeKind::Renamed,
        git::StageChange::TypeChanged => ChangeKind::TypeChanged,
        git::StageChange::Untracked => ChangeKind::Untracked,
    }
}

/// One file's resolved content, ready for rendering. Owns its
/// [`FileDiff`] so a [`Model`] is a self-contained owned value (no
/// borrow of the parsed diff), which lets the TUI replace it on reload.
#[cfg_attr(test, derive(Debug, Clone))]
pub(super) struct ResolvedFile {
    pub(super) file: FileDiff,
    pub(super) before: String,
    pub(super) after: String,
}

/// Resolve content for every file. Consumes the parsed diff (taking each
/// [`FileDiff`] by value). Returns the resolved files on success, or a
/// string describing the first missing blob on failure.
pub(super) fn resolve(
    parsed: GitDiff,
    repo: Option<&git::Repo>,
) -> Result<Vec<ResolvedFile>, String> {
    let mut files = Vec::with_capacity(parsed.files.len());

    for file in parsed.files {
        // Symlink changes are decided straight from the parsed diff and
        // never touch the ODB or the working tree (reading a link would
        // follow it to the target), so skip content resolution — they
        // can never register as a missing blob.
        if SymlinkView::from_file_diff(&file).is_some() {
            files.push(ResolvedFile {
                file,
                before: String::new(),
                after: String::new(),
            });
            continue;
        }

        // Binary changes are decided straight from the parsed diff too:
        // their blobs are not valid UTF-8, so resolving them would fail
        // and blank the whole panel. Skip content resolution entirely.
        if crate::sidebar::file_metadata(&file).binary {
            files.push(ResolvedFile {
                file,
                before: String::new(),
                after: String::new(),
            });
            continue;
        }

        // Submodule (gitlink) changes name a *commit* OID, not a blob, so
        // content resolution would fail (not a blob, and the path is a
        // directory in the working tree) and blank the whole panel. Decide
        // straight from the parsed diff, exactly like symlinks/binaries.
        if crate::sidebar::file_metadata(&file).is_submodule {
            files.push(ResolvedFile {
                file,
                before: String::new(),
                after: String::new(),
            });
            continue;
        }

        let resolved = content::retrieve(&file, repo);
        let before = match resolved.before {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(&file)));
            }
        };
        let after = match resolved.after {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(&file)));
            }
        };
        files.push(ResolvedFile {
            file,
            before,
            after,
        });
    }

    Ok(files)
}

fn missing_blob_message(hash: &str, path: &str) -> String {
    format!(
        "missing index blob {hash} for {path} \u{2014} not found in local repository\n\
         hint: fetch the source ref (e.g. `git fetch <remote> <ref>`) and try again"
    )
}

/// Decide one [`FileBody`] per resolved file. Done once at build time so
/// the diff pane and the sidebar share one decision (and the same
/// line-count totals). Symlink entries become a [`SymlinkView`]; every
/// other file is a computed text [`Diff`].
pub(super) fn precompute_bodies(files: &[ResolvedFile]) -> Vec<FileBody> {
    files
        .iter()
        .map(|f| {
            let meta = crate::sidebar::file_metadata(&f.file);
            if meta.binary {
                return FileBody::Binary;
            }
            if meta.is_submodule {
                return FileBody::Submodule {
                    old_commit: f.file.old_hash.clone(),
                    new_commit: f.file.new_hash.clone(),
                };
            }
            match SymlinkView::from_file_diff(&f.file) {
                Some(view) => FileBody::Symlink(view),
                None => FileBody::Diff(Arc::new(Diff::compute(
                    &f.before,
                    &f.after,
                    display_path(&f.file),
                ))),
            }
        })
        .collect()
}

/// Added/deleted line counts for one file body. A text diff sums its
/// hunk lines; a symlink counts one changed line per present side (an
/// added new target, a deleted old target), matching what the symlink
/// view paints.
pub(super) fn body_deltas(body: &FileBody) -> (usize, usize) {
    match body {
        FileBody::Diff(diff) => count_deltas(diff),
        FileBody::Symlink(view) => (
            view.new_target.is_some() as usize,
            view.old_target.is_some() as usize,
        ),
        // Binary changes have no line counts (lazygit shows none either).
        FileBody::Binary | FileBody::StatusOnly => (0, 0),
        // Submodule bumps have no textual line counts either.
        FileBody::Submodule { .. } => (0, 0),
    }
}

/// Sum added/deleted line counts across all hunks of one diff.
pub(super) fn count_deltas(diff: &Diff) -> (usize, usize) {
    let mut added = 0;
    let mut deleted = 0;
    for hunk in diff.hunks() {
        for line in &hunk.lines {
            match line.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => deleted += 1,
                LineKind::Context => {}
            }
        }
    }
    (added, deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::test_support::*;

    #[test]
    fn build_model_empty_input_yields_no_files() {
        let model = build_model("", None, HashMap::new()).expect("empty model");
        assert!(model.files.is_empty(), "expected zero files");
        assert!(model.bodies.is_empty(), "expected zero bodies");
    }

    #[test]
    fn count_deltas_counts_added_and_removed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "old1\nold2\nshared\n".to_string(),
            after: "new1\nshared\nnew2\n".to_string(),
        }];
        let bodies = precompute_bodies(&resolved);
        let (added, deleted) = body_deltas(&bodies[0]);
        assert!(added > 0, "expected adds");
        assert!(deleted > 0, "expected dels");
    }

    #[test]
    fn precompute_bodies_decides_symlink_from_mode() {
        let mut f = file_diff("link.txt");
        f.new_mode = Some("120000".to_string());
        let resolved = vec![ResolvedFile {
            file: f,
            before: String::new(),
            after: String::new(),
        }];
        let bodies = precompute_bodies(&resolved);
        assert!(
            matches!(bodies[0], FileBody::Symlink(_)),
            "symlink mode should yield a symlink body"
        );
    }

    #[test]
    fn missing_blob_propagates_error() {
        // Forge a diff whose old blob hash is non-null and unresolvable.
        let diff = "diff --git a/foo.txt b/foo.txt\n\
                    index deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..0000000000000000000000000000000000000000 100644\n\
                    --- a/foo.txt\n\
                    +++ /dev/null\n\
                    @@ -1 +0,0 @@\n\
                    -gone\n";
        let parsed = GitDiff::parse(diff);
        let Err(err) = resolve(parsed, None) else {
            panic!("resolve should fail on missing blob");
        };
        assert!(err.contains("missing index blob"), "got: {err}");
        assert!(err.contains("foo.txt"), "got: {err}");
    }

    #[test]
    fn build_model_keeps_binary_file_without_blanking() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        std::fs::write(dir.path().join("bin"), b"\x00\x01\x02").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // One text edit + one binary edit, both staged so the post-image
        // blobs land in the ODB.
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        std::fs::write(dir.path().join("bin"), b"\x00\x03\x04").unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .expect("binary edit must not error");

        assert_eq!(model.files.len(), 2, "both files must survive");
        let bin_idx = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == "bin")
            .expect("binary file present in model");
        assert!(
            matches!(model.bodies[bin_idx], FileBody::Binary),
            "binary file should get a Binary body"
        );
        assert_eq!(
            body_deltas(&model.bodies[bin_idx]),
            (0, 0),
            "binary file shows no +/- counts"
        );
    }

    #[test]
    fn build_model_keeps_submodule_bump_without_erroring() {
        let dir = tempfile::tempdir().unwrap();
        setup_submodule_bump(dir.path());

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        // The core guarantee: a submodule commit bump must not error the
        // whole panel (it did before the gitlink short-circuit).
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .expect("submodule bump must not error");

        let sub_idx = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == "sub")
            .expect("submodule file present in model");
        assert!(
            matches!(model.bodies[sub_idx], FileBody::Submodule { .. }),
            "submodule should get a Submodule body, got {:?}",
            std::mem::discriminant(&model.bodies[sub_idx])
        );
        assert_eq!(
            body_deltas(&model.bodies[sub_idx]),
            (0, 0),
            "submodule bump shows no +/- counts"
        );
        assert!(
            crate::sidebar::file_metadata(&model.files[sub_idx].file).is_submodule,
            "the submodule row must carry the submodule marker for its badge"
        );
    }

    #[test]
    fn build_model_from_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Stage the change so the post-image blob is in the ODB.
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();

        assert_eq!(model.files.len(), 1);
        assert_eq!(display_path(&model.files[0].file), "a.txt");
        assert_eq!(model.files[0].before, "hello\n");
        assert_eq!(model.files[0].after, "world\n");
        match &model.bodies[0] {
            FileBody::Diff(diff) => assert!(!diff.hunks().is_empty()),
            FileBody::Symlink(_)
            | FileBody::Binary
            | FileBody::StatusOnly
            | FileBody::Submodule { .. } => {
                panic!("expected a text diff body")
            }
        }
    }

    #[test]
    fn build_model_pure_rename_is_single_row_with_zero_counts() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(
            dir.path().join("old.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Pure rename: move and stage the delete + add.
        std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();

        assert_eq!(model.files.len(), 1, "rename must be a single row");
        assert_eq!(display_path(&model.files[0].file), "new.txt");
        assert_eq!(
            model.files[0].file.rename_from.as_deref(),
            Some("old.txt"),
            "rename origin must be recorded"
        );
        assert_eq!(
            body_deltas(&model.bodies[0]),
            (0, 0),
            "a pure rename shows no +/- counts"
        );

        let stage = model.stages.get("new.txt").expect("new.txt staged entry");
        assert_eq!(stage.staged, Some(ChangeKind::Renamed));
    }

    #[test]
    fn build_model_staged_type_change_is_single_row_with_counts() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
        std::fs::write(dir.path().join("target.txt"), "target content\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Regular file → symlink, staged.
        std::fs::remove_file(dir.path().join("f.txt")).unwrap();
        std::os::unix::fs::symlink("target.txt", dir.path().join("f.txt")).unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();

        assert_eq!(model.files.len(), 1, "type change must be a single row");
        assert_eq!(display_path(&model.files[0].file), "f.txt");
        assert!(
            matches!(model.bodies[0], FileBody::Diff(_)),
            "type change renders as a content diff"
        );
        let (added, deleted) = body_deltas(&model.bodies[0]);
        assert!(added > 0 && deleted > 0, "expected combined +/- counts");

        let stage = model.stages.get("f.txt").expect("f.txt staged entry");
        assert_eq!(stage.staged, Some(ChangeKind::TypeChanged));
    }

    #[test]
    fn build_model_unstaged_type_change_is_single_row() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
        std::fs::write(dir.path().join("target.txt"), "target content\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Regular file → symlink, left unstaged.
        std::fs::remove_file(dir.path().join("f.txt")).unwrap();
        std::os::unix::fs::symlink("target.txt", dir.path().join("f.txt")).unwrap();

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();

        assert_eq!(model.files.len(), 1, "type change must be a single row");
        assert_eq!(display_path(&model.files[0].file), "f.txt");

        let stage = model.stages.get("f.txt").expect("f.txt staged entry");
        assert_eq!(stage.unstaged, Some(ChangeKind::TypeChanged));
    }

    #[test]
    fn build_model_content_changed_rename_is_single_row_with_counts() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(
            dir.path().join("old.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Rename plus an edit: still one row, but with real +/- counts.
        std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        std::fs::write(
            dir.path().join("new.txt"),
            "line one\nline two CHANGED\nline three\nline four\nline five\n",
        )
        .unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(
            &input,
            Some(&wrapper),
            stage_map(wrapper.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();

        assert_eq!(model.files.len(), 1, "rename must be a single row");
        assert_eq!(display_path(&model.files[0].file), "new.txt");
        assert_eq!(
            model.files[0].file.rename_from.as_deref(),
            Some("old.txt"),
            "rename origin must be recorded"
        );
        let (added, deleted) = body_deltas(&model.bodies[0]);
        assert!(added > 0, "expected adds in content-changed rename");
        assert!(deleted > 0, "expected dels in content-changed rename");

        let stage = model.stages.get("new.txt").expect("new.txt staged entry");
        assert_eq!(stage.staged, Some(ChangeKind::Renamed));
    }
}
