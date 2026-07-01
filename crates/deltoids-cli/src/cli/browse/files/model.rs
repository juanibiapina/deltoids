//! Data axis for `review`: parse a diff, resolve before/after blob
//! content against the repo, and compute per-file [`Diff`]s. The owned
//! [`Model`] is rebuilt wholesale on each working-tree reload.

use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::{Diff, LineKind, content, git};

use crate::sidebar::display_path;

/// Describes whether and how the diff can be refreshed mid-session.
pub(super) enum DiffSource<'a> {
    /// Piped stdin: a closed stream, never refreshes.
    Static,
    /// Bare repo: re-diff the working tree when files change on disk.
    WorkingTree(&'a git::Repo),
}

/// The owned data the TUI renders: resolved files plus their diffs.
/// Rebuilt wholesale on each working-tree reload.
pub(in crate::cli::browse) struct Model {
    pub(in crate::cli::browse) files: Vec<ResolvedFile>,
    pub(super) diffs: Vec<Diff>,
}

/// Parse `input`, resolve every file's before/after content against
/// `repo`, and compute per-file [`Diff`]s.
pub(in crate::cli::browse) fn build_model(
    input: &str,
    repo: Option<&git::Repo>,
) -> Result<Model, String> {
    let parsed = GitDiff::parse(input);
    let files = resolve(parsed, repo)?;
    let diffs = precompute_diffs(&files);
    Ok(Model { files, diffs })
}

/// One file's resolved content, ready for rendering. Owns its
/// [`FileDiff`] so a [`Model`] is a self-contained owned value (no
/// borrow of the parsed diff), which lets the TUI replace it on reload.
#[cfg_attr(test, derive(Debug))]
pub(in crate::cli::browse) struct ResolvedFile {
    pub(in crate::cli::browse) file: FileDiff,
    pub(in crate::cli::browse) before: String,
    pub(in crate::cli::browse) after: String,
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

/// Compute one [`Diff`] per resolved file. Done once at startup so the
/// diff pane and the sidebar share the same line-count totals.
pub(super) fn precompute_diffs(files: &[ResolvedFile]) -> Vec<Diff> {
    files
        .iter()
        .map(|f| Diff::compute(&f.before, &f.after, display_path(&f.file)))
        .collect()
}

/// Sum added/deleted line counts across all hunks of one diff.
pub(in crate::cli::browse) fn count_deltas(diff: &Diff) -> (usize, usize) {
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
        assert!(model.diffs.is_empty(), "expected zero diffs");
    }

    #[test]
    fn count_deltas_counts_added_and_removed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "old1\nold2\nshared\n".to_string(),
            after: "new1\nshared\nnew2\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let (added, deleted) = count_deltas(&diffs[0]);
        assert!(added > 0, "expected adds");
        assert!(deleted > 0, "expected dels");
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
        assert!(!model.diffs[0].hunks().is_empty());
    }
}
