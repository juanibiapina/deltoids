//! Refresh axis: install the filesystem watcher, decide when a change
//! warrants a reload, and re-diff the working tree in place while
//! preserving the user's navigation state.

use std::path::PathBuf;

use deltoids::{Theme, git};

use crate::sidebar::display_path;

use super::diff_pane::DiffPane;
#[cfg(test)]
use super::model::build_model;
use super::model::{DiffSource, Model, stage_map};
use super::sidebar_pane::build_sidebar;
use crate::cli::browse::watch::{ChangeWatcher, path_warrants_reload, spawn_workdir_watcher};
use crate::sidebar::Sidebar;

/// Install a recursive filesystem watcher for a refreshable source.
///
/// Delegates to [`spawn_workdir_watcher`] for a
/// [`DiffSource::WorkingTree`]; a [`DiffSource::Static`] source yields no
/// watcher.
pub(super) fn spawn_watcher(source: &DiffSource<'_>) -> Result<Option<ChangeWatcher>, String> {
    match source {
        DiffSource::WorkingTree(repo) => spawn_workdir_watcher(repo),
        DiffSource::Static => Ok(None),
    }
}

/// Whether a batch of changed `paths` warrants a working-tree reload.
///
/// Only [`DiffSource::WorkingTree`] reloads; the filter itself lives in
/// [`path_warrants_reload`].
pub(super) fn should_reload(source: &DiffSource<'_>, paths: &[PathBuf]) -> bool {
    let DiffSource::WorkingTree(repo) = source else {
        return false;
    };
    path_warrants_reload(repo, paths)
}

/// Whether a freshly-computed `working_tree_diff` differs from the text
/// the current model was built from. The diff text is the source of
/// truth for diff bodies. Sidebar staging status is checked separately.
fn reload_needed(new_input: &str, last_input: &str) -> bool {
    new_input != last_input
}

/// The result of one working-tree reload tick. A working-tree diff is a
/// snapshot of a moving target, so a failed tick is transient and
/// self-correcting: the shell schedules a retry after a failed read.
/// This enum keeps that distinction visible to the caller (the mode) so a
/// failure never kills the loop and a still-loading startup can decide
/// when a run of failures is genuinely stuck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReloadOutcome {
    /// The diff changed and the view was rebuilt in place.
    Rebuilt,
    /// Only staging columns changed; diff bodies and render blocks are retained.
    StagingUpdated,
    /// The tree was stable (diff unchanged); nothing rebuilt.
    Unchanged,
    /// The tick failed (a transient diff/content race); the current view
    /// is kept untouched and the next scheduled tick will retry. Carries
    /// the error message so a still-loading startup can surface it if the
    /// failure persists past its window.
    Failed(String),
}

/// Re-diff the working tree and rebuild the view in place, preserving the
/// selected file by path. Computes a fresh model from
/// `repo.working_tree_snapshot()` via [`compute_reload`], then applies it via
/// [`apply_reload`].
///
/// Deduplicates on patch text and sidebar staging status. A notification
/// that changes neither skips the model/view rebuild.
///
/// A working-tree diff races on-disk churn: a file can change size mid-diff
/// (the libgit2 Filesystem error) or between the diff and content
/// resolution (a hash mismatch → missing blob). Both are transient and
/// self-correct, so this returns [`ReloadOutcome::Failed`] rather than an
/// error, leaving `model`/`last_input` untouched for the next tick to
/// retry.
#[allow(clippy::too_many_arguments)]
pub(super) fn reload_working_tree(
    diff: &mut DiffPane,
    sidebar: &mut Sidebar,
    model: &mut Model,
    last_input: &mut String,
    repo: &git::Repo,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) -> ReloadOutcome {
    let computed = compute_reload(repo, last_input, model);
    apply_reload(
        diff,
        sidebar,
        model,
        last_input,
        computed,
        theme,
        width,
        diff_viewport,
    )
}

enum Update {
    Staging(std::collections::HashMap<String, crate::sidebar::StageStatus>),
    Model(String, Model),
}

/// Read patch and staging columns together. Column-only changes retain the
/// model; content or membership changes build a replacement. A stable snapshot
/// returns `None`. Failed reads leave the caller's current view untouched.
fn compute_reload(
    repo: &git::Repo,
    last_input: &str,
    model: &Model,
) -> Result<Option<Update>, String> {
    let snapshot = repo.working_tree_snapshot()?;
    let stages = stage_map(&snapshot.stages);
    if !reload_needed(&snapshot.patch, last_input) {
        if stages == model.stages {
            return Ok(None);
        }
        if stages.len() == model.stages.len()
            && stages.keys().all(|path| model.stages.contains_key(path))
        {
            return Ok(Some(Update::Staging(stages)));
        }
    }
    let model = super::model::build_working_model(&snapshot, repo)?;
    Ok(Some(Update::Model(snapshot.patch, model)))
}

/// Apply staging columns or replace the content model. Column updates preserve
/// rendered diff blocks and navigation. Failed computations retain the view.
#[allow(clippy::too_many_arguments)]
fn apply_reload(
    diff: &mut DiffPane,
    sidebar: &mut Sidebar,
    model: &mut Model,
    last_input: &mut String,
    computed: Result<Option<Update>, String>,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) -> ReloadOutcome {
    let (input, new_model) = match computed {
        Ok(Some(Update::Model(input, model))) => (input, model),
        Ok(Some(Update::Staging(stages))) => {
            model.stages = stages;
            super::sidebar_pane::update_staging(sidebar, model);
            return ReloadOutcome::StagingUpdated;
        }
        Ok(None) => return ReloadOutcome::Unchanged,
        // Transient race: keep the current view (do not touch
        // model/last_input) and let the next tick retry.
        Err(msg) => return ReloadOutcome::Failed(msg),
    };
    let prev_path = sidebar
        .nearest_file_index()
        .and_then(|idx| model.files.get(idx))
        .map(|f| display_path(&f.file).to_string());
    let prev_directory = sidebar.selected_directory_path();
    let previous_order = sidebar.display_order();
    let position = sidebar
        .selection_display_range()
        .map(|r| r.start)
        .unwrap_or(0);
    let removed = prev_path.as_ref().is_some_and(|path| {
        !new_model
            .files
            .iter()
            .any(|f| display_path(&f.file) == path)
    });
    let fallback = removed
        .then(|| {
            let survivors: std::collections::HashMap<_, _> = new_model
                .files
                .iter()
                .enumerate()
                .map(|(index, file)| (display_path(&file.file), index))
                .collect();
            previous_order
                .iter()
                .skip(position + 1)
                .chain(
                    previous_order[..position.min(previous_order.len())]
                        .iter()
                        .rev(),
                )
                .filter_map(|idx| model.files.get(*idx))
                .find_map(|file| survivors.get(display_path(&file.file)).copied())
        })
        .flatten();
    reload_view(
        diff,
        sidebar,
        &new_model,
        prev_path.as_deref(),
        theme,
        width,
        diff_viewport,
    );
    let restored_directory = prev_directory
        .as_deref()
        .is_some_and(|directory| sidebar.select_directory_path(directory, diff_viewport));
    if !restored_directory && let Some(index) = fallback {
        sidebar.select_file_index(index, diff_viewport);
    }
    *model = new_model;
    *last_input = input;
    ReloadOutcome::Rebuilt
}

/// Rebuild the sidebar and diff view from `model`, preserving the user's
/// navigation state. Selection is restored by `prev_path` (index-based
/// restore would break when files are added or removed); when the file is
/// gone the fresh sidebar's default (first file) stands. Focus, sidebar
/// width, help visibility, and wheel state live on `state` and are left
/// untouched. Scroll is clamped to the new range then snapped to the
/// restored selection.
fn reload_view(
    diff: &mut DiffPane,
    sidebar: &mut Sidebar,
    model: &Model,
    prev_path: Option<&str>,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) {
    let new_sidebar = build_sidebar(model, theme);
    let display_order = new_sidebar.display_order();

    // Disk changed: drop the retained per-file blocks and rebuild lazily on
    // the next draw at the current width.
    diff.cache.clear();
    diff.display_order = display_order;
    diff.cached_width = width;
    diff.reset_window();
    *sidebar = new_sidebar;

    if let Some(path) = prev_path
        && let Some(idx) = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == path)
    {
        sidebar.select_file_index(idx, diff_viewport);
    }

    // Start from the top of the restored selection's window, keeping the
    // reviewer's cursor on the diff line it was on.
    diff.after_reload();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::comments::CommentStore;
    use crate::cli::browse::files::sidebar_pane::sidebar_footer;
    use crate::cli::browse::files::test_support::*;
    use crate::cli::browse::mode::DrawBudget;

    #[test]
    fn reload_needed_only_when_input_changed() {
        assert!(
            !reload_needed("same", "same"),
            "identical input must not trigger a rebuild"
        );
        assert!(
            reload_needed("new", "old"),
            "changed input must trigger a rebuild"
        );
        // Clearing the diff (commit that drops to the empty state).
        assert!(reload_needed("", "old diff"), "cleared diff must rebuild");
    }

    #[test]
    fn reload_preserves_selection_by_path() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt (input index 1)
        let prev = selected_path(&state, &m1);
        assert_eq!(prev.as_deref(), Some("b.txt"));

        // New model inserts a file before b.txt, shifting its index.
        let m2 = model_of(&["a.txt", "aa.txt", "b.txt", "c.txt"]);
        reload_view(
            &mut state.diff,
            &mut state.sidebar,
            &m2,
            prev.as_deref(),
            &theme(),
            80,
            4,
        );

        assert_eq!(selected_path(&state, &m2).as_deref(), Some("b.txt"));
        // The diff pane is filtered to the restored file and snapped to its top.
        let dr = state.sidebar.selection_display_range();
        state.diff.assemble_window(
            dr,
            &m2,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
            DrawBudget::Full,
            &CommentStore::default(),
        );
        assert_eq!(line_text(&state.diff.rows()[0].line), "b.txt");
        assert_eq!(state.diff.cursor.scroll, 0);
    }

    #[test]
    fn reload_to_empty_model_renders_empty_state() {
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let empty = Model {
            files: Vec::new(),
            bodies: Vec::new(),
            stages: Default::default(),
        };
        reload_view(
            &mut state.diff,
            &mut state.sidebar,
            &empty,
            Some("a.txt"),
            &theme(),
            80,
            4,
        );

        assert!(state.diff.display_order.is_empty());
        let dr = state.sidebar.selection_display_range();
        state.diff.assemble_window(
            dr,
            &empty,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
            DrawBudget::Full,
            &CommentStore::default(),
        );
        assert!(state.diff.rows().is_empty());
        assert_eq!(state.diff.window_rows(), 0);
        assert_eq!(
            sidebar_footer(&state.sidebar, &state.diff.display_order),
            None
        );
        assert_eq!(state.diff.footer(), None);
    }

    #[test]
    fn reload_clamps_selection_when_file_disappears() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt

        // b.txt is gone (reverted/committed); selection must clamp.
        let m2 = model_of(&["a.txt", "c.txt"]);
        reload_view(
            &mut state.diff,
            &mut state.sidebar,
            &m2,
            Some("b.txt"),
            &theme(),
            80,
            4,
        );

        let path = selected_path(&state, &m2);
        assert!(
            matches!(path.as_deref(), Some("a.txt") | Some("c.txt")),
            "selection should clamp to a surviving file, got {path:?}"
        );
    }

    /// A model with the given file paths, for feeding `apply_reload`.
    fn apply_reload_computed(paths: &[&str]) -> Result<Option<Update>, String> {
        Ok(Some(Update::Model(
            format!("diff for {paths:?}"),
            model_of(paths),
        )))
    }

    #[test]
    fn apply_reload_failed_keeps_model_and_last_input() {
        // A failed compute (either the diff read or the model build lost a
        // race) must keep the current view: no model swap, no last_input
        // change, and a Failed outcome so the caller can retry.
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let mut model = model_of(&["a.txt", "b.txt"]);
        let mut last_input = "original diff".to_string();

        let outcome = apply_reload(
            &mut state.diff,
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            Err("file changed before we could read it".to_string()),
            &theme(),
            80,
            4,
        );

        assert!(matches!(outcome, ReloadOutcome::Failed(_)));
        assert_eq!(last_input, "original diff", "last_input must be untouched");
        assert_eq!(model.files.len(), 2, "model must be untouched");
    }

    #[test]
    fn apply_reload_unchanged_keeps_model_and_last_input() {
        // A stable tree (diff unchanged) is a no-op that keeps the view.
        let m1 = model_of(&["a.txt"]);
        let mut state = make_state(&m1.files);
        let mut model = model_of(&["a.txt"]);
        let mut last_input = "original diff".to_string();

        let outcome = apply_reload(
            &mut state.diff,
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            Ok(None),
            &theme(),
            80,
            4,
        );

        assert_eq!(outcome, ReloadOutcome::Unchanged);
        assert_eq!(last_input, "original diff");
        assert_eq!(model.files.len(), 1);
    }

    #[test]
    fn apply_reload_rebuilt_swaps_model_and_last_input() {
        // A fresh model swaps in and updates last_input.
        let m1 = model_of(&["a.txt"]);
        let mut state = make_state(&m1.files);
        let mut model = model_of(&["a.txt"]);
        let mut last_input = "original diff".to_string();

        let outcome = apply_reload(
            &mut state.diff,
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            apply_reload_computed(&["a.txt", "b.txt"]),
            &theme(),
            80,
            4,
        );

        assert_eq!(outcome, ReloadOutcome::Rebuilt);
        assert_ne!(last_input, "original diff", "last_input must advance");
        assert_eq!(model.files.len(), 2, "model must swap to the new one");
    }

    #[test]
    fn static_source_never_reloads() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_reload(
            &DiffSource::Static,
            &[dir.path().join("src/main.rs")]
        ));
    }
    #[test]
    fn staging_refreshes_sidebar_status_when_patch_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let raw = git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("file.txt"), "before\n").unwrap();
        let mut index = raw.index().unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = raw.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        raw.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        std::fs::write(dir.path().join("file.txt"), "after\n").unwrap();
        raw.blob(b"after\n").unwrap();
        let repo = git::Repo::discover_at(dir.path()).unwrap();
        let patch = repo.working_tree_diff().unwrap();
        let model = build_model(
            &patch,
            Some(&repo),
            stage_map(repo.working_tree_snapshot().unwrap().stages),
        )
        .unwrap();
        assert!(compute_reload(&repo, &patch, &model).unwrap().is_none());

        index.read(true).unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let Update::Staging(stages) = compute_reload(&repo, &patch, &model).unwrap().unwrap()
        else {
            panic!("staging must not rebuild the diff model");
        };
        assert_ne!(stages, model.stages);
    }
}
