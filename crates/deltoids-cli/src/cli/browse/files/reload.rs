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

/// The patch text a model's bodies come from: the HEAD → worktree patch
/// plus the staged and unstaged patches of files with both columns.
/// Staging part of such a file leaves the net patch unchanged, so all
/// three take part in change detection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::cli::browse::files) struct Patches {
    all: String,
    staged: String,
    unstaged: String,
}

impl Patches {
    pub(super) fn of(snapshot: &git::WorkingTreeSnapshot) -> Self {
        Self {
            all: snapshot.patch.clone(),
            staged: snapshot.staged_patch.clone(),
            unstaged: snapshot.unstaged_patch.clone(),
        }
    }
}

#[cfg(test)]
impl Patches {
    /// Patches with only a HEAD -> worktree part.
    pub(super) fn net(all: &str) -> Self {
        Self {
            all: all.to_string(),
            ..Self::default()
        }
    }

    pub(super) fn all(&self) -> &str {
        &self.all
    }
}

/// Whether freshly-read patches differ from the ones the current model was
/// built from. The patches are the source of truth for diff bodies.
/// Sidebar staging status is checked separately.
fn reload_needed(new_input: &Patches, last_input: &Patches) -> bool {
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
    /// Only staging columns of single-column files changed; diff bodies
    /// and render blocks are retained, and files may change panes.
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
pub(super) fn reload_working_tree(
    panes: [&mut DiffPane; 2],
    sidebar: &mut Sidebar,
    model: &mut Model,
    last_input: &mut Patches,
    repo: &git::Repo,
    theme: &Theme,
    diff_viewport: usize,
) -> ReloadOutcome {
    let computed = compute_reload(repo, last_input, model);
    apply_reload(
        panes,
        sidebar,
        model,
        last_input,
        computed,
        theme,
        diff_viewport,
    )
}

enum Update {
    Staging(std::collections::HashMap<String, crate::sidebar::StageStatus>),
    Model(Patches, Model),
}

/// Read patch and staging columns together. Column-only changes retain the
/// model; content or membership changes build a replacement. A stable snapshot
/// returns `None`. Failed reads leave the caller's current view untouched.
fn compute_reload(
    repo: &git::Repo,
    last_input: &Patches,
    model: &Model,
) -> Result<Option<Update>, String> {
    let snapshot = repo.working_tree_snapshot()?;
    let stages = stage_map(&snapshot.stages);
    let patches = Patches::of(&snapshot);
    if !reload_needed(&patches, last_input) {
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
    Ok(Some(Update::Model(patches, model)))
}

/// Apply staging columns or replace the content model. Column updates preserve
/// rendered diff blocks and navigation. Failed computations retain the view.
fn apply_reload(
    panes: [&mut DiffPane; 2],
    sidebar: &mut Sidebar,
    model: &mut Model,
    last_input: &mut Patches,
    computed: Result<Option<Update>, String>,
    theme: &Theme,
    diff_viewport: usize,
) -> ReloadOutcome {
    let (input, mut new_model) = match computed {
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
    new_model.keep_unchanged_bodies(model);
    reload_view(
        panes,
        sidebar,
        model,
        &new_model,
        prev_path.as_deref(),
        theme,
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
/// gone the fresh sidebar's default (first file) stands. Folded
/// directories that still exist stay folded. Focus, sidebar
/// width, help visibility, and wheel state live on `state` and are left
/// untouched. Each pane keeps the rendered blocks of files that did not
/// change, and its scroll when the file at the top of the viewport is one
/// of them.
fn reload_view(
    panes: [&mut DiffPane; 2],
    sidebar: &mut Sidebar,
    old: &Model,
    model: &Model,
    prev_path: Option<&str>,
    theme: &Theme,
    diff_viewport: usize,
) {
    for pane in panes {
        pane.carry_over(&old.unchanged_views(model, pane.column()));
    }
    let folds = sidebar.folds();
    *sidebar = build_sidebar(model, theme);
    sidebar.apply_folds(folds, diff_viewport);

    if let Some(path) = prev_path
        && let Some(idx) = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == path)
    {
        sidebar.select_file_index(idx, diff_viewport);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::FilesMode;
    use crate::cli::browse::files::model::ResolvedFile;
    use crate::cli::browse::files::sidebar_pane::sidebar_footer;
    use crate::cli::browse::files::test_support::*;

    #[test]
    fn reload_needed_only_when_input_changed() {
        let net = Patches::net;
        assert!(
            !reload_needed(&net("same"), &net("same")),
            "identical input must not trigger a rebuild"
        );
        assert!(
            reload_needed(&net("new"), &net("old")),
            "changed input must trigger a rebuild"
        );
        // Clearing the diff (commit that drops to the empty state).
        assert!(
            reload_needed(&net(""), &net("old diff")),
            "cleared diff must rebuild"
        );
        // Staging part of a file with both columns leaves the net patch alone.
        let staged = Patches {
            staged: "more staged".to_string(),
            ..net("same")
        };
        assert!(
            reload_needed(&staged, &net("same")),
            "a changed split patch must rebuild"
        );
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
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &m1,
            &m2,
            prev.as_deref(),
            &theme(),
            4,
        );

        assert_eq!(selected_path(&state, &m2).as_deref(), Some("b.txt"));
        // The diff pane is filtered to the restored file and snapped to its top.
        assemble_unstaged(
            &mut state.unstaged,
            &m2,
            &state.sidebar,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
        );
        assert_eq!(line_text(&state.unstaged.rows()[0].line), "b.txt");
        assert_eq!(state.unstaged.cursor.scroll, 0);
    }

    #[test]
    fn reload_keeps_folds_and_selects_the_folded_directory_of_a_hidden_file() {
        let m1 = model_of(&["src/a.txt", "src/b.txt", "z.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.set_selected(0, 4);
        state.sidebar.toggle_selected_dir(4);

        let m2 = model_of(&["src/a.txt", "src/b.txt", "src/c.txt", "z.txt"]);
        reload_view(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &m1,
            &m2,
            Some("src/a.txt"),
            &theme(),
            4,
        );

        assert_eq!(state.sidebar.row_count(), 2);
        assert_eq!(
            state.sidebar.selected_directory_path().as_deref(),
            Some("src/")
        );
        assert_eq!(state.sidebar.selection_display_range(), Some(0..3));
    }

    #[test]
    fn reload_to_empty_model_renders_empty_state() {
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let empty = Model::empty();
        reload_view(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &m1,
            &empty,
            Some("a.txt"),
            &theme(),
            4,
        );

        assert!(state.sidebar.display_order().is_empty());
        assemble_unstaged(
            &mut state.unstaged,
            &empty,
            &state.sidebar,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
        );
        assert!(state.unstaged.rows().is_empty());
        assert_eq!(state.unstaged.window_rows(), 0);
        assert_eq!(
            sidebar_footer(&state.sidebar, &state.sidebar.display_order()),
            None
        );
        assert_eq!(state.unstaged.footer(), None);
    }

    #[test]
    fn reload_clamps_selection_when_file_disappears() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt

        // b.txt is gone (reverted/committed); selection must clamp.
        let m2 = model_of(&["a.txt", "c.txt"]);
        reload_view(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &m1,
            &m2,
            Some("b.txt"),
            &theme(),
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
            Patches::net(&format!("diff for {paths:?}")),
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
        let mut last_input = Patches::net("original diff");

        let outcome = apply_reload(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            Err("file changed before we could read it".to_string()),
            &theme(),
            4,
        );

        assert!(matches!(outcome, ReloadOutcome::Failed(_)));
        assert_eq!(
            last_input.all(),
            "original diff",
            "last_input must be untouched"
        );
        assert_eq!(model.files.len(), 2, "model must be untouched");
    }

    #[test]
    fn apply_reload_unchanged_keeps_model_and_last_input() {
        // A stable tree (diff unchanged) is a no-op that keeps the view.
        let m1 = model_of(&["a.txt"]);
        let mut state = make_state(&m1.files);
        let mut model = model_of(&["a.txt"]);
        let mut last_input = Patches::net("original diff");

        let outcome = apply_reload(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            Ok(None),
            &theme(),
            4,
        );

        assert_eq!(outcome, ReloadOutcome::Unchanged);
        assert_eq!(last_input.all(), "original diff");
        assert_eq!(model.files.len(), 1);
    }

    #[test]
    fn apply_reload_rebuilt_swaps_model_and_last_input() {
        // A fresh model swaps in and updates last_input.
        let m1 = model_of(&["a.txt"]);
        let mut state = make_state(&m1.files);
        let mut model = model_of(&["a.txt"]);
        let mut last_input = Patches::net("original diff");

        let outcome = apply_reload(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &mut model,
            &mut last_input,
            apply_reload_computed(&["a.txt", "b.txt"]),
            &theme(),
            4,
        );

        assert_eq!(outcome, ReloadOutcome::Rebuilt);
        assert_ne!(last_input.all(), "original diff", "last_input must advance");
        assert_eq!(model.files.len(), 2, "model must swap to the new one");
    }

    /// Reload `state` onto a model built from `files`, then assemble the
    /// unstaged window as the next draw would, without waiting for renders.
    fn reload_onto(state: &mut FilesMode, files: &[ResolvedFile]) {
        let outcome = apply_reload(
            [&mut state.staged, &mut state.unstaged],
            &mut state.sidebar,
            &mut state.model,
            &mut state.last_input,
            Ok(Some(Update::Model(
                Patches::net(&format!("{:?}", files.len())),
                model_from(files),
            ))),
            &theme(),
            4,
        );
        assert_eq!(outcome, ReloadOutcome::Rebuilt);
        assemble_unstaged(
            &mut state.unstaged,
            &state.model,
            &state.sidebar,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
        );
    }

    /// Assemble the unstaged window and wait until every block is rendered.
    fn settle(state: &mut FilesMode) {
        for _ in 0..2 {
            assemble_unstaged(
                &mut state.unstaged,
                &state.model,
                &state.sidebar,
                80,
                deltoids::ChangeLayout::Grouped,
                &theme(),
            );
            wait_for_render(&mut state.unstaged);
        }
        assemble_unstaged(
            &mut state.unstaged,
            &state.model,
            &state.sidebar,
            80,
            deltoids::ChangeLayout::Grouped,
            &theme(),
        );
    }

    fn top_line(state: &FilesMode) -> String {
        line_text(&state.unstaged.rows()[state.unstaged.cursor.scroll].line)
    }

    fn is_rendering(state: &FilesMode) -> bool {
        state
            .unstaged
            .rows()
            .iter()
            .any(|row| line_text(&row.line).contains("Rendering"))
    }

    #[test]
    fn reload_keeps_scroll_when_only_another_file_changed() {
        let b = long_file("b.txt", 40);
        let mut state = make_state(&[b.clone(), resolved("c.txt")]);
        settle(&mut state);
        state.unstaged.scroll_by(20, 4);
        let before = top_line(&state);

        let mut c = resolved("c.txt");
        c.after.push_str("more\n");
        reload_onto(&mut state, &[resolved("a.txt"), b, c]);

        assert!(!is_rendering(&state), "b.txt must come from the cache");
        assert_eq!(state.unstaged.cursor.scroll, 20);
        assert_eq!(top_line(&state), before);
    }

    #[test]
    fn reload_resets_scroll_when_the_shown_file_changed() {
        let mut state = make_state(&[long_file("b.txt", 40), resolved("c.txt")]);
        settle(&mut state);
        state.unstaged.scroll_by(20, 4);

        reload_onto(&mut state, &[long_file("b.txt", 41), resolved("c.txt")]);

        assert_eq!(state.unstaged.cursor.scroll, 0);
    }

    #[test]
    fn directory_reload_keeps_the_top_file_when_an_earlier_file_grows() {
        let mut state = make_state(&[long_file("src/a.txt", 10), long_file("src/b.txt", 40)]);
        state.sidebar.set_selected(0, 4);
        settle(&mut state);
        let b_start = state
            .unstaged
            .rows()
            .iter()
            .position(|row| line_text(&row.line) == "src/b.txt")
            .unwrap();
        state.unstaged.scroll_by((b_start + 10) as isize, 4);
        let before = top_line(&state);

        reload_onto(
            &mut state,
            &[long_file("src/a.txt", 25), long_file("src/b.txt", 40)],
        );
        settle(&mut state);

        assert_eq!(top_line(&state), before);
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
        let patches = Patches::net(&patch);
        assert!(compute_reload(&repo, &patches, &model).unwrap().is_none());

        index.read(true).unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let Update::Staging(stages) = compute_reload(&repo, &patches, &model).unwrap().unwrap()
        else {
            panic!("staging must not rebuild the diff model");
        };
        assert_ne!(stages, model.stages);
    }
}
