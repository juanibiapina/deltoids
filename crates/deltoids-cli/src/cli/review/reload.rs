//! Refresh axis: install the filesystem watcher, decide when a change
//! warrants a reload, and re-diff the working tree in place while
//! preserving the user's navigation state.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use deltoids::{Theme, git};

use crate::sidebar::display_path;

use super::ViewState;
use super::diff_pane::{DiffPane, build_view};
use super::model::{DiffSource, Model, build_model};
use super::sidebar_pane::build_sidebar;

/// Install a recursive filesystem watcher for a refreshable source.
///
/// The callback only forwards each event's paths over the returned
/// channel: git2's `Repository` is `!Send` and can't move into the (Send)
/// closure, so gitignore filtering stays on the main thread. For a static
/// source (or a bare repo with no workdir) no watcher is created and the
/// receiver never fires (the sender is dropped here).
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
    let (notify_tx, notify_rx) = mpsc::channel::<Vec<PathBuf>>();
    let watcher = match source {
        DiffSource::WorkingTree(repo) => match repo.workdir() {
            Some(workdir) => {
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if let Ok(event) = res {
                            let _ = notify_tx.send(event.paths);
                        }
                    })
                    .map_err(|err| format!("failed to create filesystem watcher: {err}"))?;
                watcher
                    .watch(workdir, RecursiveMode::Recursive)
                    .map_err(|err| format!("failed to watch {}: {err}", workdir.display()))?;
                Some(watcher)
            }
            None => None,
        },
        DiffSource::Static => None,
    };
    Ok((watcher, notify_rx))
}

/// Whether a batch of changed `paths` warrants a working-tree reload.
///
/// Only [`DiffSource::WorkingTree`] reloads. A path counts when it is
/// neither inside `.git/` (git's constant index/lock churn) nor
/// gitignored (ignored files never appear in `working_tree_diff`, so a
/// change there can't alter the diff). Fails open via
/// [`git::Repo::is_ignored`], so a real change is never missed.
pub(super) fn should_reload(source: &DiffSource<'_>, paths: &[PathBuf]) -> bool {
    let DiffSource::WorkingTree(repo) = source else {
        return false;
    };
    paths
        .iter()
        .any(|path| !is_git_internal(path) && !repo.is_ignored(path))
}

/// Whether `path` lies inside a `.git` directory.
fn is_git_internal(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
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
pub(super) fn reload_working_tree(
    state: &mut ViewState,
    model: &mut Model,
    repo: &git::Repo,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
    last_input: &mut String,
) -> Result<bool, String> {
    let input = repo.working_tree_diff()?;
    if !reload_needed(&input, last_input) {
        return Ok(false);
    }
    let prev_path = state
        .sidebar
        .nearest_file_index()
        .and_then(|idx| model.files.get(idx))
        .map(|f| display_path(&f.file).to_string());
    let new_model = build_model(&input, Some(repo))?;
    reload_view(
        state,
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
    state: &mut ViewState,
    model: &Model,
    prev_path: Option<&str>,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) {
    let sidebar = build_sidebar(model, theme);
    let display_order = sidebar.display_order();
    let view = build_view(&model.files, &model.diffs, &display_order, width, theme);

    state.diff = DiffPane {
        diff_lines: view.lines,
        file_offsets: view.file_offsets,
        display_order,
        cached_width: width,
        diff_scroll: state.diff.diff_scroll,
    };
    state.sidebar = sidebar;

    if let Some(path) = prev_path
        && let Some(idx) = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == path)
    {
        state.sidebar.select_file_index(idx, diff_viewport);
    }

    let dr = state.sidebar.selection_display_range();
    let min = state.diff.min_scroll(dr.clone());
    let max = state.diff.max_scroll(dr, diff_viewport);
    state.diff.diff_scroll = state.diff.diff_scroll.clamp(min, max.max(min));
    state.snap_diff_to_selected_file(diff_viewport);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::review::sidebar_pane::sidebar_footer;
    use crate::cli::review::test_support::*;

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
        reload_view(&mut state, &m2, prev.as_deref(), &theme(), 80, 4);

        assert_eq!(selected_path(&state, &m2).as_deref(), Some("b.txt"));
        // The diff pane is filtered to the restored file.
        let range = state.visible_diff_range();
        assert_eq!(line_text(&state.diff.diff_lines[range.start]), "b.txt");
        assert!(range.contains(&state.diff.diff_scroll) || state.diff.diff_scroll == range.start);
    }

    #[test]
    fn reload_to_empty_model_renders_empty_state() {
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let empty = Model {
            files: Vec::new(),
            diffs: Vec::new(),
        };
        reload_view(&mut state, &empty, Some("a.txt"), &theme(), 80, 4);

        assert!(state.diff.diff_lines.is_empty());
        assert_eq!(state.visible_diff_range(), 0..0);
        assert_eq!(
            sidebar_footer(&state.sidebar, &state.diff.display_order),
            None
        );
        assert_eq!(
            state.diff.footer(state.sidebar.selection_display_range()),
            None
        );
    }

    #[test]
    fn reload_clamps_selection_when_file_disappears() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt

        // b.txt is gone (reverted/committed); selection must clamp.
        let m2 = model_of(&["a.txt", "c.txt"]);
        reload_view(&mut state, &m2, Some("b.txt"), &theme(), 80, 4);

        let path = selected_path(&state, &m2);
        assert!(
            matches!(path.as_deref(), Some("a.txt") | Some("c.txt")),
            "selection should clamp to a surviving file, got {path:?}"
        );
    }

    #[test]
    fn should_reload_filters_ignored_and_git_internal() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let source = DiffSource::WorkingTree(&wrapper);

        assert!(should_reload(&source, &[dir.path().join("src/main.rs")]));
        assert!(!should_reload(
            &source,
            &[dir.path().join("node_modules/x.js")]
        ));
        assert!(!should_reload(
            &source,
            &[dir.path().join(".git/index.lock")]
        ));
        // A batch with at least one real path still reloads.
        assert!(should_reload(
            &source,
            &[
                dir.path().join(".git/index"),
                dir.path().join("src/main.rs"),
            ]
        ));
        // A static source never reloads.
        assert!(!should_reload(
            &DiffSource::Static,
            &[dir.path().join("src/main.rs")]
        ));
    }

    #[test]
    fn is_git_internal_detects_dot_git() {
        assert!(is_git_internal(Path::new("/repo/.git/index")));
        assert!(is_git_internal(Path::new(".git/HEAD")));
        assert!(!is_git_internal(Path::new("/repo/src/main.rs")));
    }
}
