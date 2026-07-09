//! Data axis for `review`: parse a diff, resolve before/after blob
//! content against the repo, and compute per-file [`Diff`]s. The owned
//! [`Model`] is rebuilt wholesale on each working-tree reload.

use std::collections::HashMap;

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
pub(super) enum FileBody {
    Diff(Diff),
    Symlink(SymlinkView),
    /// A binary change: no textual diff. Decided from the parsed diff, so
    /// content resolution never touches the ODB or the working tree.
    Binary,
}

/// Parse `input`, resolve every file's before/after content against
/// `repo`, and compute per-file [`Diff`]s.
pub(super) fn build_model(input: &str, repo: Option<&git::Repo>) -> Result<Model, String> {
    let parsed = GitDiff::parse(input);
    let files = resolve(parsed, repo)?;
    let bodies = precompute_bodies(&files);
    let stages = stage_map(repo);
    Ok(Model {
        files,
        bodies,
        stages,
    })
}

/// Query the repo's per-file staging status and index it by path for
/// the sidebar join. Returns an empty map with no repo or on any git
/// error (the sidebar then falls back to single-letter status).
pub(super) fn stage_map(repo: Option<&git::Repo>) -> HashMap<String, StageStatus> {
    let Some(repo) = repo else {
        return HashMap::new();
    };
    let Ok(statuses) = repo.working_tree_status() else {
        return HashMap::new();
    };
    statuses
        .into_iter()
        .map(|s| {
            (
                s.path,
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
            if crate::sidebar::file_metadata(&f.file).binary {
                return FileBody::Binary;
            }
            match SymlinkView::from_file_diff(&f.file) {
                Some(view) => FileBody::Symlink(view),
                None => FileBody::Diff(Diff::compute(&f.before, &f.after, display_path(&f.file))),
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
        FileBody::Binary => (0, 0),
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
        let model = build_model("", None).expect("empty model");
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
    fn stage_map_joins_stage_status_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // a.txt: unstaged modify. b.txt: staged add.
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("b.txt")).unwrap();
            index.write().unwrap();
        }

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let stages = stage_map(Some(&wrapper));

        let a = stages.get("a.txt").expect("a.txt staged entry");
        assert_eq!(a.staged, None);
        assert_eq!(a.unstaged, Some(ChangeKind::Modified));

        let b = stages.get("b.txt").expect("b.txt staged entry");
        assert_eq!(b.staged, Some(ChangeKind::Added));
        assert_eq!(b.unstaged, None);
    }

    #[test]
    fn stage_map_empty_without_repo() {
        assert!(stage_map(None).is_empty());
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
        let model = build_model(&input, Some(&wrapper)).expect("binary edit must not error");

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
        let model = build_model(&input, Some(&wrapper)).unwrap();

        assert_eq!(model.files.len(), 1);
        assert_eq!(display_path(&model.files[0].file), "a.txt");
        assert_eq!(model.files[0].before, "hello\n");
        assert_eq!(model.files[0].after, "world\n");
        match &model.bodies[0] {
            FileBody::Diff(diff) => assert!(!diff.hunks().is_empty()),
            FileBody::Symlink(_) | FileBody::Binary => panic!("expected a text diff body"),
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
        let model = build_model(&input, Some(&wrapper)).unwrap();

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
        let model = build_model(&input, Some(&wrapper)).unwrap();

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
        let model = build_model(&input, Some(&wrapper)).unwrap();

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
        let model = build_model(&input, Some(&wrapper)).unwrap();

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
