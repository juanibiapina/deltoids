//! Refresh axis: install the filesystem watcher, decide when a change
//! warrants a reload, and re-diff the working tree in place while
//! preserving the user's navigation state.

use std::path::PathBuf;
use std::sync::mpsc;

use deltoids::{Theme, git};

use crate::sidebar::display_path;

use super::diff_pane::DiffPane;
use super::model::{DiffSource, Model, build_model};
use super::sidebar_pane::build_sidebar;
use crate::cli::browse::watch::{path_warrants_reload, spawn_workdir_watcher};
use crate::sidebar::Sidebar;

/// Install a recursive filesystem watcher for a refreshable source.
///
/// Delegates to [`spawn_workdir_watcher`] for a
/// [`DiffSource::WorkingTree`]; a [`DiffSource::Static`] source yields no
/// watcher and a receiver that never fires (the sender is dropped here).
#[allow(clippy::type_complexity)]
pub(super) fn spawn_watcher(
    source: &DiffSource<'_>,
) -> Result<
    (
        Option<notify::RecommendedWatcher>,
        mpsc::Receiver<Vec<PathBuf>>,
    ),
    String,
> {
    match source {
        DiffSource::WorkingTree(repo) => spawn_workdir_watcher(repo),
        DiffSource::Static => Ok((None, mpsc::channel().1)),
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
/// truth, so this is the one check that decides whether a poll tick or
/// filesystem event warrants a rebuild.
fn reload_needed(new_input: &str, last_input: &str) -> bool {
    new_input != last_input
}

/// Re-diff the working tree and rebuild the view in place, preserving the
/// selected file by path. Captures the current selection's path from the
/// old `model`, builds a fresh model from `repo.working_tree_diff()`,
/// applies it via [`reload_view`], then swaps `model` to the new value.
///
/// Deduplicates on the diff text: `last_input` holds what the current
/// model was built from, so an unchanged tree (a poll tick or a
/// filesystem event that doesn't alter the diff) costs one
/// `working_tree_diff` and skips the model/view rebuild. Returns `true`
/// only when it rebuilt.
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
) -> Result<bool, String> {
    let input = repo.working_tree_diff()?;
    if !reload_needed(&input, last_input) {
        return Ok(false);
    }
    let prev_path = sidebar
        .nearest_file_index()
        .and_then(|idx| model.files.get(idx))
        .map(|f| display_path(&f.file).to_string());
    let new_model = build_model(&input, Some(repo))?;
    reload_view(
        diff,
        sidebar,
        &new_model,
        prev_path.as_deref(),
        theme,
        width,
        diff_viewport,
    );
    *model = new_model;
    *last_input = input;
    Ok(true)
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
    diff.window_rows = 0;
    *sidebar = new_sidebar;

    if let Some(path) = prev_path
        && let Some(idx) = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == path)
    {
        sidebar.select_file_index(idx, diff_viewport);
    }

    // Snap to the top of the restored selection's window.
    diff.snap_to_top();
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let window = state
            .diff
            .assemble_window(dr, &m2, 80, &theme(), DrawBudget::Full);
        assert_eq!(line_text(&window[0]), "b.txt");
        assert_eq!(state.diff.diff_scroll, 0);
    }

    #[test]
    fn reload_to_empty_model_renders_empty_state() {
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let empty = Model {
            files: Vec::new(),
            diffs: Vec::new(),
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
        let window = state
            .diff
            .assemble_window(dr, &empty, 80, &theme(), DrawBudget::Full);
        assert!(window.is_empty());
        assert_eq!(state.diff.window_rows, 0);
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

    #[test]
    fn static_source_never_reloads() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_reload(
            &DiffSource::Static,
            &[dir.path().join("src/main.rs")]
        ));
    }
}
