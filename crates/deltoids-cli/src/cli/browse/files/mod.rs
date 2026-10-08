//! Files mode: the working-tree view of the unified TUI.
//!
//! Discovers the repository and shows its local working-tree changes
//! against `HEAD`. The working tree is watched and re-diffed on
//! change. Not-a-repo and a clean tree both degrade to an empty
//! "No local changes." state, so the TUI still opens.
//!
//! A working-tree diff is a snapshot of a moving target, so both the
//! startup build and every reload can lose a race with on-disk churn.
//! Neither is fatal:
//!
//! - **Reload** keeps the current view on a failed tick (see
//!   [`reload::ReloadOutcome`]); explicit retries recover it, so it
//!   self-heals.
//! - **Startup** inside a repo shows a neutral "Loading…" state and
//!   builds a normal, reloadable mode: the first successful tick promotes
//!   it to a live diff. Only a failure that persists past a short window
//!   degrades to the static error state, so a real error never hides
//!   behind an endless spinner.
//!
//! Layout: a file-tree sidebar (left column) and the deltoids diff
//! renderer (right). Selecting a file scrolls the diff to it. The diff
//! shows one staging column at a time: staged (HEAD → index) or unstaged
//! (index → worktree). When the selection has both, `s` switches between
//! them; [`stage_panes`] decides which column shows which files.
//!
//! ## Module layout
//!
//! Split by change axis. This file is the mode adapter: it owns the
//! mode's state, its key/mouse handling, its render, and its live
//! reload, and implements [`super::mode::Mode`]. Each pane owns its
//! vertical slice:
//!
//! - [`model`]: the data axis: parse/resolve/diff.
//! - [`diff_pane`]: the diff pane's state, scroll math, keys, render.
//! - [`stage_panes`]: which staging column the diff shows, with which files.
//! - [`sidebar_pane`]: the sidebar's build, keys, render, footer.
//! - [`reload`]: the working-tree watcher and in-place rebuild.

use crate::cli::browse::watch::{
    ChangeReceiver, ChangeWatcher, path_warrants_reload, spawn_workdir_watcher,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use deltoids::{ChangeLayout, Diff, LineKind, Theme, git};

use crate::cli::browse::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar::{Sidebar, display_path};

use super::comment_view::render_comment_editor;
use super::comments::{CommentAnchor, CommentStore, reanchor, review_text};
use super::mode::{AppCommand, BackgroundWork, DrawBudget, Mode, Viewport};

mod action_menu;
mod actions;
mod diff_pane;
#[cfg(test)]
mod interaction_tests;
mod model;
#[cfg(test)]
mod performance_tests;
mod reload;
mod render;
mod review;
#[cfg(test)]
mod review_tests;
mod sidebar_pane;
mod stage_panes;
#[cfg(test)]
mod test_support;

use diff_pane::{DiffPane, HunkLabels, SCROLL_STEP_LARGE, SCROLL_STEP_SMALL};
#[cfg(test)]
use model::build_model;
use model::{Column, Model};
use reload::{Patches, ReloadOutcome, reload_working_tree};
use sidebar_pane::build_sidebar;
pub(crate) use sidebar_pane::sidebar_title;
use stage_panes::{PaneSpec, PaneTitle, StagePanes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Focus {
    Sidebar,
    Diff,
}

/// Whether the diff pane is being browsed or a comment is being typed.
#[derive(Debug)]
enum InputState {
    Normal,
    Discarding {
        menu: action_menu::DiscardMenu,
        selected: usize,
    },
    /// The comment editor is open on `anchor`, holding the text so far
    /// plus the line it annotates. The snapshot is taken when the editor
    /// opens, so saving still works when the working tree changes
    /// underneath while the reviewer types.
    Commenting {
        anchor: CommentAnchor,
        line: (String, LineKind),
        buffer: String,
    },
}

/// How long a repo-backed startup may stay in the "Loading…" state before
/// a still-failing build degrades to the static error screen. Under normal
/// churn a stable moment (and a successful diff) arrives well within this
/// window; only a genuinely persistent error keeps failing past it. Kept
/// generous so a burst of transient races never trips it.
const STARTUP_LOADING_TIMEOUT: Duration = Duration::from_secs(1);

/// Files-mode state plus the data it renders. Owns the model, the repo
/// (for blob resolution and reload), and the reload bookkeeping; the
/// shell owns sidebar width, help, and the divider.
pub(super) struct FilesMode {
    /// Diff pane of the staged column (HEAD → index).
    staged: DiffPane,
    /// Diff pane of the unstaged column (index → worktree); also the only
    /// pane when there is no staging information.
    unstaged: DiffPane,
    /// File indices in sidebar (display) order.
    display_order: Vec<usize>,
    /// Which files each column's pane lists. Rebuilt with the model and
    /// whenever staging columns change.
    panes: StagePanes,
    /// The column to show when the selection has both; `s` switches it.
    column: Column,
    /// Sidebar pane state: rows, selection, scroll.
    sidebar: Sidebar,
    /// Currently-focused pane within this mode.
    focus: Focus,
    /// Last-drawn pane rects, used for mouse hit-testing.
    sidebar_rect: Rect,
    diff_rect: Rect,
    /// Translates fanned-out mouse-wheel events into proportional motion.
    wheel: WheelScroll<Focus>,
    /// Session-only review comments on the working-tree diff. Survives
    /// reloads: anchors are line-based, not index-based.
    comments: CommentStore,
    input: InputState,
    /// Transient diff-pane footer message (copy feedback).
    status: Option<String>,
    /// The owned data: resolved files plus their diffs.
    model: Model,
    /// The repo (for blob resolution and working-tree reload), if any.
    repo: Option<git::Repo>,
    /// True while a repo-backed startup is still resolving its first diff
    /// (the initial build lost a race): the pane shows "Loading…" and the
    /// first successful reload promotes it to a live view. `loading_since`
    /// bounds how long this may persist before degrading to a static
    /// error.
    startup_pending: bool,
    /// When the startup "Loading…" state began, used to bound it: a
    /// failure past [`STARTUP_LOADING_TIMEOUT`] degrades to a static error
    /// rather than spinning forever. `None` once resolved or never
    /// pending.
    loading_since: Option<Instant>,
    /// The patches the current model was built from; event-driven reloads
    /// compare fresh output against these and check sidebar staging status.
    last_input: Patches,
    /// Keeps the bounded filesystem watcher alive for the session.
    _watcher: Option<ChangeWatcher>,
    reload_failed: bool,
    action_job: Option<std::sync::mpsc::Receiver<Result<actions::DiscardOutcome, String>>>,
    refresh_requested: bool,
    review: review::Review,
    guidance: review::Guidance,
    hide_low: bool,
}

impl FilesMode {
    /// Build the mode from the discovered repo's working tree, always
    /// yielding a renderable mode. Not-a-repo and a clean tree render the
    /// empty "No local changes." state; a repo-backed build that loses a
    /// race renders a neutral, reloadable "Loading…" state that self-heals
    /// on the first successful tick. `initial_diff_width` seeds the diff
    /// cache for the first frame.
    pub(super) fn discover(theme: &Theme, initial_diff_width: usize) -> Self {
        // Not a repo: degrade to the static empty state so the TUI still
        // opens.
        let Some(repo) = git::Repo::discover() else {
            return Self::empty(theme, initial_diff_width);
        };
        // Watch before the snapshot so edits during the build stay pending.
        let watcher = crate::cli::browse::watch::spawn_workdir_watcher(&repo)
            .ok()
            .flatten();
        let mut mode = match Self::try_model(&repo) {
            Ok((input, model)) => Self::new(model, input, Some(repo), theme, initial_diff_width),
            // A repo-backed build lost a race (a size-check or content
            // race). Rather than a static error screen, open a reloadable
            // "Loading…" mode; explicit retries and the first
            // stable tick promotes it to a live diff.
            Err(_) => Self::loading(repo, theme, initial_diff_width),
        };
        mode._watcher = watcher;
        mode.review = review::Review::from_env();
        mode.start_review();
        mode
    }

    /// Compute the working-tree diff and its model, or an error when the
    /// diff read or model build loses a race with on-disk churn.
    fn try_model(repo: &git::Repo) -> Result<(Patches, Model), String> {
        let snapshot = repo.working_tree_snapshot()?;
        let model = model::build_working_model(&snapshot, repo)?;
        Ok((Patches::of(&snapshot), model))
    }

    /// A cheap empty Files mode: no repo, no diff, static. Used as the
    /// startup placeholder for the inactive mode and as the not-a-repo
    /// fallback.
    pub(super) fn empty(theme: &Theme, width: usize) -> Self {
        Self::new(Model::empty(), Patches::default(), None, theme, width)
    }

    /// A reloadable Files mode that shows a neutral "Loading…" state while
    /// a repo-backed startup resolves its first diff. It holds the repo
    /// and an empty model with a sentinel `last_input`, so the ordinary
    /// reload path rebuilds it into a live view on the first stable tick;
    /// [`FilesMode::reload`] promotes it out of Loading, or degrades it to
    /// the static error state once failures persist past
    /// [`STARTUP_LOADING_TIMEOUT`].
    fn loading(repo: git::Repo, theme: &Theme, width: usize) -> Self {
        let mut mode = Self::new(Model::empty(), Patches::default(), Some(repo), theme, width);
        mode.startup_pending = true;
        mode.loading_since = Some(Instant::now());
        mode.unstaged.set_empty_loading();
        mode
    }

    /// A Files mode that shows a build-error message instead of the empty
    /// state. Holds no repo and is static: reserved for a genuinely
    /// non-recoverable failure (and the startup Loading guard, which
    /// degrades to it once a build error persists past the loading
    /// window), so this mode is never watched or reloaded.
    pub(super) fn error(theme: &Theme, width: usize, message: String) -> Self {
        let mut mode = Self::new(Model::empty(), Patches::default(), None, theme, width);
        mode.unstaged.set_empty_error(message);
        mode
    }

    fn new(
        model: Model,
        input: Patches,
        repo: Option<git::Repo>,
        theme: &Theme,
        width: usize,
    ) -> Self {
        let sidebar = build_sidebar(&model, theme);
        let display_order = sidebar.display_order();
        let panes = StagePanes::new(&model, &display_order);
        let mut mode = Self {
            staged: DiffPane::new(Column::Staged, width),
            unstaged: DiffPane::new(Column::Unstaged, width),
            display_order,
            panes,
            column: Column::Unstaged,
            sidebar,
            focus: Focus::Sidebar,
            sidebar_rect: Rect::default(),
            diff_rect: Rect::default(),
            wheel: WheelScroll::new(),
            comments: CommentStore::default(),
            input: InputState::Normal,
            status: None,
            model,
            repo,
            startup_pending: false,
            loading_since: None,
            last_input: input,
            _watcher: None,
            reload_failed: false,
            action_job: None,
            refresh_requested: false,
            review: review::Review::default(),
            guidance: review::Guidance::default(),
            hide_low: false,
        };
        mode.start_review();
        mode
    }

    fn pane(&self, column: Column) -> &DiffPane {
        match column {
            Column::Staged => &self.staged,
            Column::Unstaged => &self.unstaged,
        }
    }

    fn pane_mut(&mut self, column: Column) -> &mut DiffPane {
        match column {
            Column::Staged => &mut self.staged,
            Column::Unstaged => &mut self.unstaged,
        }
    }

    /// The pane the current selection shows.
    fn spec(&self) -> PaneSpec<'_> {
        self.panes
            .view(self.sidebar.selection_display_range(), self.column)
    }

    /// The column the diff pane shows right now.
    fn shown(&self) -> Column {
        self.spec().column
    }

    /// Switch to the other column when the selection has both.
    fn toggle_column(&mut self) {
        if let PaneTitle::Both(shown) = self.spec().title {
            self.column = match shown {
                Column::Staged => Column::Unstaged,
                Column::Unstaged => Column::Staged,
            };
        }
    }

    fn start_review(&mut self) {
        self.review.request(&self.model);
        self.refresh_guidance();
        self.sidebar.update_notes(&self.guidance.rows);
    }

    fn refresh_guidance(&mut self) {
        self.guidance = self.review.guidance(&self.model);
    }

    fn hidden(&self) -> Vec<bool> {
        if self.hide_low {
            self.guidance.low.clone()
        } else {
            Vec::new()
        }
    }

    fn rebuild_sidebar(&mut self, height: usize) {
        let directory = self.sidebar.selected_directory_path();
        let file = self.sidebar.nearest_file_index();
        let folds = self.sidebar.folds();
        self.sidebar = sidebar_pane::rebuild_sidebar(&self.sidebar, &self.model, &self.hidden());
        self.sidebar.apply_folds(folds, height);
        let restored = directory
            .as_deref()
            .is_some_and(|path| self.sidebar.select_directory_path(path, height));
        if !restored && let Some(index) = file {
            self.sidebar.select_file_index(index, height);
        }
        self.sidebar.update_notes(&self.guidance.rows);
        self.refresh_panes();
    }

    fn toggle_hidden(&mut self, height: usize) {
        self.hide_low = !self.hide_low;
        self.rebuild_sidebar(height);
        self.snap_diff_to_selected_file();
    }

    /// Reports whether anything changed on screen.
    fn collect_review(&mut self) -> bool {
        let Some(result) = self.review.collect() else {
            return false;
        };
        self.status = result
            .err()
            .map(|error| format!("Jev unavailable: {error}"));
        self.refresh_guidance();
        if self.hide_low {
            let height = self.sidebar_rect.height.saturating_sub(2) as usize;
            self.rebuild_sidebar(height);
        } else {
            self.sidebar.update_notes(&self.guidance.rows);
        }
        true
    }

    fn sidebar_footer(&self) -> Option<String> {
        sidebar_pane::sidebar_footer(&self.sidebar, &self.display_order)
    }

    /// Re-sort files into columns after the model or its staging changed.
    fn refresh_panes(&mut self) {
        self.display_order = self.sidebar.display_order();
        self.panes = StagePanes::new(&self.model, &self.display_order);
    }

    /// Show `column` and assemble its diff window, as a draw does. Empty
    /// when the selection has no `column` changes.
    #[cfg(test)]
    fn visible_window(
        &mut self,
        column: Column,
        budget: DrawBudget,
    ) -> Vec<ratatui::text::Line<'static>> {
        self.column = column;
        if self.shown() != column {
            return Vec::new();
        }
        loop {
            let width = self.pane(column).cached_width;
            self.assemble(
                column,
                width,
                ChangeLayout::Grouped,
                &Theme::default(),
                budget,
            );
            let pane = self.pane_mut(column);
            if budget != DrawBudget::Full || !pane.cache.pending() {
                return pane.rows().iter().map(|row| row.line.clone()).collect();
            }
            test_support::wait_for_render(pane);
        }
    }

    /// Assemble `column`'s pane, as a draw does, when the selection shows it.
    #[cfg(test)]
    fn assemble(
        &mut self,
        column: Column,
        width: usize,
        layout: ChangeLayout,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        let spec = self
            .panes
            .view(self.sidebar.selection_display_range(), self.column);
        if spec.column != column {
            return;
        }
        let pane = match column {
            Column::Staged => &mut self.staged,
            Column::Unstaged => &mut self.unstaged,
        };
        let column = spec.column;
        let (model, review) = (&self.model, &self.review);
        let lookup = |file: usize, hunk: usize| review.hunk_label(model, column, file, hunk);
        pane.assemble_window(
            &spec,
            &self.model,
            width,
            layout,
            theme,
            budget,
            &self.comments,
            &HunkLabels {
                revision: self.review.revision(),
                lookup: &lookup,
            },
        );
    }

    /// Assemble the unstaged (or only) diff window.
    #[cfg(test)]
    fn visible_diff_window(&mut self, budget: DrawBudget) -> Vec<ratatui::text::Line<'static>> {
        self.visible_window(Column::Unstaged, budget)
    }

    /// Sync the diff panes' scroll to the top of the selected file's
    /// window (a sidebar move re-derives the window).
    fn snap_diff_to_selected_file(&mut self) {
        self.staged.snap_to_top();
        self.unstaged.snap_to_top();
    }

    /// Fold a [`ReloadOutcome`] into the startup Loading state machine and
    /// report whether the visible content changed (the redraw signal).
    ///
    /// A successful tick (rebuilt or a confirmed-stable tree) promotes a
    /// still-loading startup to its live view. A failed tick keeps the
    /// current view; while loading, it counts against the loading window
    /// and, once the window elapses, degrades to the static error screen so
    /// a persistent error never hides behind an endless "Loading…".
    fn resolve_reload(&mut self, outcome: ReloadOutcome, theme: &Theme, width: usize) -> bool {
        match outcome {
            ReloadOutcome::Rebuilt | ReloadOutcome::StagingUpdated => {
                self.promote_from_loading();
                true
            }
            ReloadOutcome::Unchanged => {
                // A successful diff that matched `last_input`. At startup
                // (`last_input == ""`) this confirms a clean tree, so leave
                // Loading for the "No local changes." state.
                let changed = self.startup_pending;
                self.promote_from_loading();
                changed
            }
            ReloadOutcome::Failed(msg) => {
                // A build error that persists past the loading window is
                // treated as genuine: degrade to the static error screen
                // (repo dropped, no reload). A fresh failure keeps Loading.
                if self.startup_pending && self.loading_window_elapsed() {
                    // The user's notes outlive the view they were made on.
                    let comments = std::mem::take(&mut self.comments);
                    *self = Self::error(theme, width, msg);
                    self.comments = comments;
                    return true;
                }
                false
            }
        }
    }

    /// Whether the startup "Loading…" window has elapsed. A `None`
    /// `loading_since` (not loading) reports elapsed, but the caller only
    /// consults this while `startup_pending`.
    fn loading_window_elapsed(&self) -> bool {
        self.loading_since
            .map(|since| since.elapsed() >= STARTUP_LOADING_TIMEOUT)
            .unwrap_or(true)
    }

    /// Leave the startup "Loading…" state once a tick resolves it. Clears
    /// the pending flag and the loading window and resets the empty-pane
    /// render to the neutral "No local changes." state (irrelevant once
    /// files are present). A no-op when not loading.
    fn promote_from_loading(&mut self) {
        if self.startup_pending {
            self.startup_pending = false;
            self.loading_since = None;
            self.unstaged.clear_empty_state();
        }
    }
}

fn start_stage_all(state: &mut FilesMode) {
    if state.action_job.is_some() {
        return;
    }
    let Some(workdir) = state
        .repo
        .as_ref()
        .and_then(|r| r.workdir())
        .map(PathBuf::from)
    else {
        state.status = Some("Git actions require a working tree".into());
        return;
    };
    state.status = Some("Updating staging for all files…".into());
    spawn_action(state, move || {
        actions::toggle_stage_all(&workdir).map(actions::DiscardOutcome::Applied)
    });
}

fn start_action(state: &mut FilesMode, stage: bool) {
    if !stage {
        state.input = InputState::Discarding {
            menu: action_menu::DiscardMenu::Checking,
            selected: 2,
        };
    }
    let unavailable = |state: &mut FilesMode, reason: &str| {
        if !stage {
            state.input = InputState::Discarding {
                menu: action_menu::DiscardMenu::Unavailable(reason.into()),
                selected: 2,
            };
        }
    };
    if state.action_job.is_some() {
        unavailable(
            state,
            "A Git action is already running; reopen the menu when it finishes",
        );
        return;
    }
    let Some(range) = state.sidebar.selection_display_range() else {
        unavailable(state, "No files selected");
        return;
    };
    let Some(workdir) = state
        .repo
        .as_ref()
        .and_then(|r| r.workdir())
        .map(PathBuf::from)
    else {
        let reason = "Git actions require a working tree";
        unavailable(state, reason);
        if stage {
            state.status = Some(reason.into());
        }
        return;
    };
    let targets: std::collections::BTreeSet<_> = range
        .filter_map(|position| {
            state
                .display_order
                .get(position)
                .and_then(|idx| state.model.files.get(*idx))
        })
        .flat_map(|file| [&file.file.old_path, &file.file.new_path])
        .filter(|path| path.as_str() != "/dev/null")
        .map(PathBuf::from)
        .collect();
    state.status = Some(
        if stage {
            "Updating staging…"
        } else {
            "Checking discard…"
        }
        .into(),
    );
    spawn_action(state, move || {
        let targets = targets.into_iter().collect();
        if stage {
            actions::toggle_stage(&workdir, targets).map(actions::DiscardOutcome::Applied)
        } else {
            actions::discard(&workdir, targets)
        }
    });
}

fn spawn_action(
    state: &mut FilesMode,
    action: impl FnOnce() -> Result<actions::DiscardOutcome, String> + Send + 'static,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    state.action_job = Some(receiver);
    std::thread::spawn(move || {
        let _ = sender.send(action());
    });
}

fn collect_action(state: &mut FilesMode) -> bool {
    use std::sync::mpsc::TryRecvError;
    let Some(receiver) = &state.action_job else {
        return false;
    };
    let result = match receiver.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return false,
        Err(TryRecvError::Disconnected) => Err("Git action worker stopped".into()),
    };
    state.action_job = None;
    state.status = None;
    match result {
        Ok(actions::DiscardOutcome::Choose(pending)) => {
            if let InputState::Discarding {
                menu: menu @ action_menu::DiscardMenu::Checking,
                selected,
            } = &mut state.input
            {
                *selected = actions::DiscardKind::ALL
                    .iter()
                    .position(|kind| pending.is_available(*kind))
                    .unwrap_or(2);
                *menu = action_menu::DiscardMenu::Ready(pending);
            }
        }
        Ok(actions::DiscardOutcome::Applied(message)) => {
            state.status = (!message.is_empty()).then_some(message);
            state.refresh_requested = true;
        }
        Err(message) => {
            if let InputState::Discarding {
                menu: menu @ action_menu::DiscardMenu::Checking,
                ..
            } = &mut state.input
            {
                *menu = action_menu::DiscardMenu::Unavailable(message.clone());
            }
            state.status = Some(format!("Git action failed: {message}"));
            state.refresh_requested = true;
        }
    }
    true
}

fn handle_discard_key(state: &mut FilesMode, key: KeyCode) -> AppCommand {
    let InputState::Discarding { menu, selected } = &mut state.input else {
        return AppCommand::Continue;
    };
    let cancel = actions::DiscardKind::ALL.len();
    match key {
        KeyCode::Char('j') | KeyCode::Down => *selected = (*selected + 1).min(cancel),
        KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
        KeyCode::Esc => state.input = InputState::Normal,
        KeyCode::Enter => {
            let kind = actions::DiscardKind::ALL.get(*selected).copied();
            if kind.is_some_and(|kind| !menu.is_available(kind)) {
                return AppCommand::Continue;
            }
            if let InputState::Discarding {
                menu: action_menu::DiscardMenu::Ready(pending),
                ..
            } = std::mem::replace(&mut state.input, InputState::Normal)
                && let Some(kind) = kind
            {
                state.status = Some("Discarding changes…".into());
                spawn_action(state, move || {
                    actions::complete_discard(pending, kind).map(actions::DiscardOutcome::Applied)
                });
            }
        }
        _ => {}
    }
    AppCommand::Continue
}

/// Handle a mode-internal key (the shell strips global bindings first).
fn handle_key(state: &mut FilesMode, key: KeyCode, height: usize) -> AppCommand {
    if matches!(state.input, InputState::Commenting { .. }) {
        return handle_comment_key(state, key);
    }

    if matches!(state.input, InputState::Discarding { .. }) {
        return handle_discard_key(state, key);
    }

    if state.action_job.is_none() {
        state.status = None;
    }
    if key == KeyCode::Char('f') {
        state.toggle_hidden(height);
        return AppCommand::Continue;
    }
    if state.focus == Focus::Sidebar && key == KeyCode::Char('a') {
        start_stage_all(state);
        return AppCommand::Continue;
    }
    if state.focus == Focus::Sidebar && matches!(key, KeyCode::Char(' ' | 'd')) {
        start_action(state, key == KeyCode::Char(' '));
        return AppCommand::Continue;
    }

    match key {
        KeyCode::Char('c') => {
            if state.action_job.is_none() {
                open_comment(state);
            }
            AppCommand::Continue
        }
        KeyCode::Char('d') => {
            delete_comment(state);
            AppCommand::Continue
        }
        KeyCode::Char('y') => copy_comments(state),
        KeyCode::Char('D') => clear_comments(state),
        KeyCode::Char('s') => {
            state.toggle_column();
            AppCommand::Continue
        }
        KeyCode::Char('z') => {
            match state.focus {
                Focus::Diff => reshape_hunk(state, Diff::expand),
                Focus::Sidebar => reshape_selection(state, Diff::expand_all),
            }
            AppCommand::Continue
        }
        KeyCode::Char('x') => {
            match state.focus {
                Focus::Diff => reshape_hunk(state, Diff::shrink),
                Focus::Sidebar => reshape_selection(state, Diff::shrink_all),
            }
            AppCommand::Continue
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Sidebar => Focus::Diff,
                Focus::Diff => Focus::Sidebar,
            };
            AppCommand::Continue
        }
        KeyCode::Char('1') => {
            state.focus = Focus::Sidebar;
            AppCommand::Continue
        }
        KeyCode::Char('2') => {
            state.focus = Focus::Diff;
            AppCommand::Continue
        }
        // Shift+J/K always scroll the diff regardless of focus.
        KeyCode::Char('J') => {
            let column = state.shown();
            state
                .pane_mut(column)
                .scroll_by(SCROLL_STEP_LARGE as isize, height);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            let column = state.shown();
            state
                .pane_mut(column)
                .scroll_by(-(SCROLL_STEP_LARGE as isize), height);
            AppCommand::Continue
        }
        // Remaining nav keys route to the focused pane. A sidebar move
        // also snaps the diff (cross-pane coordination owned here).
        other => {
            match state.focus {
                Focus::Sidebar => {
                    if sidebar_pane::handle_key(&mut state.sidebar, other, height) {
                        state.snap_diff_to_selected_file();
                    }
                }
                Focus::Diff => {
                    let column = state.shown();
                    state.pane_mut(column).handle_key(other, height);
                }
            }
            AppCommand::Continue
        }
    }
}

/// Expand or shrink the hunk under the diff cursor with `reshape`
/// ([`Diff::expand`] or [`Diff::shrink`]). Both panes re-render the file
/// (a single-column file's body is shared by both columns); the cursor
/// stays on its file line.
fn reshape_hunk(state: &mut FilesMode, reshape: fn(&Diff, usize) -> Option<Diff>) {
    let column = state.shown();
    let Some((place, anchor)) = state
        .pane(column)
        .cursor_line()
        .map(|(place, anchor)| (place.clone(), anchor.clone()))
    else {
        return;
    };
    let Some(index) = state
        .model
        .files
        .iter()
        .position(|file| display_path(&file.file) == place.file)
    else {
        return;
    };
    if state
        .model
        .reshape(index, column, |diff| reshape(diff, place.hunk))
    {
        state.pane_mut(column).reshape_file(index, anchor);
        let other = match column {
            Column::Staged => Column::Unstaged,
            Column::Unstaged => Column::Staged,
        };
        state.pane_mut(other).cache.refresh(index);
    }
}

/// Expand or shrink every hunk of every file the diff pane shows for the
/// sidebar selection with `reshape` ([`Diff::expand_all`] or
/// [`Diff::shrink_all`]), in the column the pane shows. The pane keeps
/// its scroll: its window is rebuilt from the new blocks around the same
/// file offset.
fn reshape_selection(state: &mut FilesMode, reshape: fn(&Diff) -> Option<Diff>) {
    let spec = state.spec();
    let column = spec.column;
    let Some(range) = spec.range.clone() else {
        return;
    };
    let files = spec.order[range].to_vec();
    for index in files {
        if state.model.reshape(index, column, reshape) {
            state.staged.cache.refresh(index);
            state.unstaged.cache.refresh(index);
        }
    }
}

/// Handle a key while the comment editor is open. The shell routes every
/// key here (see [`Mode::captures_text_input`]), so characters that are
/// normally global bindings are typed as text.
fn handle_comment_key(state: &mut FilesMode, key: KeyCode) -> AppCommand {
    match key {
        KeyCode::Esc => {
            state.input = InputState::Normal;
        }
        KeyCode::Enter => {
            if let InputState::Commenting {
                anchor,
                line: (code, kind),
                buffer,
            } = std::mem::replace(&mut state.input, InputState::Normal)
            {
                state.comments.set(anchor, buffer, code, kind);
            }
        }
        KeyCode::Backspace => {
            if let InputState::Commenting { buffer, .. } = &mut state.input {
                buffer.pop();
            }
        }
        KeyCode::Char(ch) => {
            if let InputState::Commenting { buffer, .. } = &mut state.input {
                buffer.push(ch);
            }
        }
        _ => {}
    }
    AppCommand::Continue
}

/// Open the comment editor for the diff line under the cursor, seeded
/// with any note already saved there. A no-op unless the diff pane is
/// focused with the cursor on a diff line.
fn open_comment(state: &mut FilesMode) {
    if state.focus != Focus::Diff {
        return;
    }
    let Some(anchor) = state.pane(state.shown()).cursor_anchor().cloned() else {
        return;
    };
    let Some(line) = anchored_line(&state.model, &anchor) else {
        return;
    };
    let buffer = state.comments.note(&anchor).unwrap_or_default().to_string();
    state.input = InputState::Commenting {
        anchor,
        line,
        buffer,
    };
}

/// Delete the comment on the diff line under the cursor, if any.
fn delete_comment(state: &mut FilesMode) {
    if state.focus != Focus::Diff {
        return;
    }
    let Some(anchor) = state.pane(state.shown()).cursor_anchor().cloned() else {
        return;
    };
    state.comments.remove(&anchor);
}

/// Store `note` against `anchor`, snapshotting the line it annotates.
/// An empty note deletes the comment. A no-op when the line is no longer
/// in the diff.
#[cfg(test)]
fn save_comment(state: &mut FilesMode, anchor: CommentAnchor, note: String) {
    let Some((code, kind)) = anchored_line(&state.model, &anchor) else {
        return;
    };
    state.comments.set(anchor, note, code, kind);
}

/// The text and kind of the diff line `anchor` points at, or `None` when
/// the file or the line is no longer in the diff.
fn anchored_line(model: &Model, anchor: &CommentAnchor) -> Option<(String, LineKind)> {
    model
        .line(anchor)
        .map(|(content, kind)| (content.to_string(), kind.clone()))
}

fn copy_comments(state: &mut FilesMode) -> AppCommand {
    match working_tree_review(state) {
        Some((text, count)) => {
            let noun = if count == 1 { "comment" } else { "comments" };
            state.status = Some(format!("Copied {count} {noun}"));
            AppCommand::CopyToClipboard(text)
        }
        None => {
            state.status = Some("No comments to copy".to_string());
            AppCommand::Continue
        }
    }
}

fn clear_comments(state: &mut FilesMode) -> AppCommand {
    let count = state.comments.clear();
    state.status = Some(if count == 0 {
        "No comments to clear".to_string()
    } else {
        let noun = if count == 1 { "comment" } else { "comments" };
        format!("Cleared {count} {noun}")
    });
    AppCommand::Continue
}

fn working_tree_review(state: &FilesMode) -> Option<(String, usize)> {
    let sections = state.model.sections(&state.display_order);
    review_text("", &state.comments, &sections)
}

fn pane_at(state: &FilesMode, col: u16, row: u16) -> Option<Focus> {
    let pos = Position::new(col, row);
    if state.sidebar_rect.contains(pos) {
        Some(Focus::Sidebar)
    } else if state.diff_rect.contains(pos) {
        Some(Focus::Diff)
    } else {
        None
    }
}

fn handle_mouse(state: &mut FilesMode, mouse: MouseEvent, height: usize) -> AppCommand {
    // The comment editor owns input while it is open.
    if !matches!(state.input, InputState::Normal) {
        return AppCommand::Continue;
    }
    // Ghostty/macOS sends Shift+vertical wheel as horizontal wheel events.
    // Both shapes step through files; Ctrl+vertical wheel does too.
    let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
    let direction = match mouse.kind {
        MouseEventKind::ScrollDown => Some(ScrollDir::Down),
        MouseEventKind::ScrollUp => Some(ScrollDir::Up),
        MouseEventKind::ScrollRight if shift => Some(ScrollDir::Down),
        MouseEventKind::ScrollLeft if shift => Some(ScrollDir::Up),
        _ => None,
    };
    let modified = shift || mouse.modifiers.contains(KeyModifiers::CONTROL);
    let list_kind = if shift
        && matches!(
            mouse.kind,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
        ) {
        ScrollKind::Discrete
    } else {
        ScrollKind::List
    };
    let target = if direction.is_some() && modified {
        Focus::Sidebar
    } else {
        match pane_at(state, mouse.column, mouse.row) {
            Some(pane) => pane,
            None => return AppCommand::Continue,
        }
    };

    match direction {
        Some(ScrollDir::Down) => match target {
            Focus::Sidebar => {
                let steps = state.wheel.advance(target, ScrollDir::Down, list_kind);
                for _ in 0..steps {
                    state.sidebar.move_down(height);
                    state.snap_diff_to_selected_file();
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let column = state.shown();
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::Content);
                state
                    .pane_mut(column)
                    .scroll_by((steps * SCROLL_STEP_SMALL) as isize, height);
                AppCommand::Continue
            }
        },
        Some(ScrollDir::Up) => match target {
            Focus::Sidebar => {
                let steps = state.wheel.advance(target, ScrollDir::Up, list_kind);
                for _ in 0..steps {
                    state.sidebar.move_up(height);
                    state.snap_diff_to_selected_file();
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let column = state.shown();
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Up, ScrollKind::Content);
                state
                    .pane_mut(column)
                    .scroll_by(-((steps * SCROLL_STEP_SMALL) as isize), height);
                AppCommand::Continue
            }
        },
        None if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
            state.focus = target;
            match target {
                Focus::Sidebar => {
                    let rect = state.sidebar_rect;
                    let content_y = mouse.row.saturating_sub(rect.y).saturating_sub(1) as usize;
                    let clicked = state.sidebar.scroll() + content_y;
                    if clicked < state.sidebar.row_count() {
                        state.sidebar.set_selected(clicked, height);
                        state.snap_diff_to_selected_file();
                    }
                }
                // Clicking a diff line moves the cursor there; clicks on
                // headers, spacers, wrapped continuations, and comment
                // rows only focus the pane.
                Focus::Diff => {
                    let content_y = mouse
                        .row
                        .saturating_sub(state.diff_rect.y)
                        .saturating_sub(1) as usize;
                    let column = state.shown();
                    let pane = state.pane_mut(column);
                    pane.select_row(pane.cursor.scroll + content_y);
                }
            }
            AppCommand::Continue
        }
        _ => AppCommand::Continue,
    }
}

impl Mode for FilesMode {
    fn build(&mut self, theme: &Theme, diff_width: usize) {
        *self = Self::discover(theme, diff_width);
    }

    fn background(&mut self, active: bool) -> BackgroundWork {
        let action_changed = collect_action(self) | self.collect_review();
        if !active {
            self.staged.cache.pause();
            self.unstaged.cache.pause();
            return BackgroundWork::default();
        }
        let staged_changed = self.staged.cache.collect();
        let unstaged_changed = self.unstaged.cache.collect();
        BackgroundWork {
            changed: staged_changed || unstaged_changed || action_changed,
            pending: self.staged.cache.pending()
                || self.unstaged.cache.pending()
                || self.action_job.is_some()
                || self.review.pending(),
        }
    }

    fn take_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.refresh_requested)
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        key == KeyCode::Char('f')
            || self.focus == Focus::Sidebar && matches!(key, KeyCode::Char(' ' | 'd' | 'a'))
    }

    fn draw(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        left: Rect,
        right: Rect,
        layout: ChangeLayout,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        self.sidebar_rect = left;
        self.diff_rect = right;

        let sidebar_focused = self.focus == Focus::Sidebar;
        sidebar_pane::draw_sidebar(
            frame,
            left,
            &self.sidebar,
            self.sidebar_footer(),
            sidebar_focused,
            theme,
        );

        let spec = self
            .panes
            .view(self.sidebar.selection_display_range(), self.column);
        let pane = match spec.column {
            Column::Staged => &mut self.staged,
            Column::Unstaged => &mut self.unstaged,
        };
        let width = super::diff_scrollbar::body_area(right).width as usize;
        let column = spec.column;
        let (model, review) = (&self.model, &self.review);
        let lookup = |file: usize, hunk: usize| review.hunk_label(model, column, file, hunk);
        pane.assemble_window(
            &spec,
            &self.model,
            width,
            layout,
            theme,
            budget,
            &self.comments,
            &HunkLabels {
                revision: self.review.revision(),
                lookup: &lookup,
            },
        );
        pane.render(
            frame,
            right,
            self.focus == Focus::Diff,
            theme,
            self.status.as_deref(),
        );

        if let InputState::Commenting { anchor, buffer, .. } = &self.input {
            let label = format!("{}:{}", anchor.path, anchor.line);
            render_comment_editor(frame, right, &label, buffer, theme);
        }
        if let InputState::Discarding { menu, selected } = &self.input {
            action_menu::draw(frame, menu, *selected, theme);
        }
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        handle_key(self, key, height)
    }

    fn captures_text_input(&self) -> bool {
        !matches!(self.input, InputState::Normal)
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        if let Err(err) = result {
            self.status = Some(format!("Copy failed: {err}"));
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        handle_mouse(self, mouse, height)
    }

    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String> {
        let Some(repo) = self.repo.as_ref() else {
            return Ok(None);
        };
        if !self
            ._watcher
            .as_ref()
            .is_some_and(ChangeWatcher::is_healthy)
        {
            self._watcher = spawn_workdir_watcher(repo)?;
            if let Some(watcher) = &self._watcher {
                watcher.receiver().request_rescan();
            }
        }
        Ok(self._watcher.as_ref().map(ChangeWatcher::receiver))
    }

    fn should_reload(&self, paths: &[PathBuf]) -> bool {
        self.repo
            .as_ref()
            .is_some_and(|repo| path_warrants_reload(repo, paths))
    }

    fn retry_reload(&self) -> bool {
        self.startup_pending || self.reload_failed
    }

    fn reload(&mut self, viewport: Viewport, theme: &Theme) -> Result<bool, String> {
        let Some(repo) = self.repo.as_ref() else {
            return Ok(false);
        };
        // `repo` borrows `self.repo`; the rest are disjoint fields.
        let outcome = reload_working_tree(
            [&mut self.staged, &mut self.unstaged],
            &mut self.sidebar,
            &mut self.model,
            &mut self.last_input,
            repo,
            theme,
            viewport.height,
        );
        self.reload_failed = matches!(outcome, ReloadOutcome::Failed(_));
        if matches!(
            outcome,
            ReloadOutcome::Rebuilt | ReloadOutcome::StagingUpdated
        ) {
            self.refresh_panes();
        }
        if outcome == ReloadOutcome::Rebuilt {
            self.review.request(&self.model);
            self.refresh_guidance();
            if self.hide_low {
                self.rebuild_sidebar(viewport.height);
            } else {
                self.sidebar.update_notes(&self.guidance.rows);
            }
        }
        // A rebuilt diff may have shifted the lines comments point at:
        // follow them onto their new numbers before anything renders.
        if outcome == ReloadOutcome::Rebuilt {
            let sections = self.model.sections(&self.display_order);
            reanchor(&mut self.comments, &sections);
        }
        let width = if viewport.diff_width > 0 {
            viewport.diff_width
        } else {
            self.unstaged.cached_width
        };
        // The reload tick is non-fatal: never return `Err`. Fold the
        // outcome into the startup Loading state machine and report only
        // whether the visible content changed.
        Ok(self.resolve_reload(outcome, theme, width))
    }

    fn selected_path(&self) -> Option<PathBuf> {
        // The selected file's workdir-relative path, joined onto the
        // repo's working directory. `None` without a repo or when there is no file under the selection.
        let idx = self.sidebar.nearest_file_index()?;
        let file = &self.model.files.get(idx)?.file;
        let rel = crate::sidebar::display_path(file);
        let workdir = self.repo.as_ref()?.workdir()?;
        Some(workdir.join(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::comments::LineSide;
    use crate::cli::browse::files::diff_pane::CacheEpoch;
    use crate::cli::browse::files::test_support::*;
    use model::ResolvedFile;

    fn large_file(path: &str) -> ResolvedFile {
        ResolvedFile {
            file: file_diff(path),
            before: String::new(),
            after: (0..10_000).map(|line| format!("line {line}\n")).collect(),
        }
    }

    #[test]
    fn navigation_away_from_pending_large_render_shows_the_latest_file() {
        let mut state = make_state(&[large_file("a.txt"), edited("b.txt")]);
        let initial = state.visible_diff_window(DrawBudget::Fast);
        assert!(
            initial
                .iter()
                .any(|line| line_text(line).contains("Rendering"))
        );
        handle_key(&mut state, KeyCode::Char('j'), 18);
        let immediate = state.visible_diff_window(DrawBudget::Fast);
        assert_eq!(line_text(&immediate[0]), "b.txt");
        wait_for_render(&mut state.unstaged);
        let ready = state.visible_diff_window(DrawBudget::Fast);
        assert!(
            ready
                .iter()
                .any(|line| line_text(line).contains("const x = 2"))
        );
        assert!(
            !ready
                .iter()
                .any(|line| line_text(line).contains("line 9999"))
        );
    }

    #[test]
    fn reload_discards_pending_content_even_when_file_indices_are_reused() {
        let mut state = make_state(&[large_file("a.txt")]);
        state.visible_diff_window(DrawBudget::Fast);
        state.unstaged.cache.clear();
        state.unstaged.reset_window();
        state.model = model_from(&[edited("a.txt")]);
        let ready = state.visible_diff_window(DrawBudget::Full);
        assert!(
            ready
                .iter()
                .any(|line| line_text(line).contains("const x = 2"))
        );
        assert!(
            !ready
                .iter()
                .any(|line| line_text(line).contains("line 9999"))
        );
    }

    #[test]
    fn hidden_pending_render_resumes_without_another_navigation_key() {
        let mut state = make_state(&[large_file("a.txt")]);
        state.visible_diff_window(DrawBudget::Fast);
        assert!(state.background(true).pending);
        let paused = state.background(false);
        assert!(!paused.pending && !paused.changed);
        state.visible_diff_window(DrawBudget::Fast);
        wait_for_render(&mut state.unstaged);
        let ready = state.visible_diff_window(DrawBudget::Fast);
        assert!(
            ready
                .iter()
                .any(|line| line_text(line).contains("line 9999"))
        );
        assert!(!state.background(true).pending);
    }

    #[test]
    fn render_settings_preserve_the_selected_line_while_replacement_rows_are_pending() {
        let mut state = diff_state(&[edited("app.rs")]);
        handle_key(&mut state, KeyCode::Char('j'), 18);
        handle_key(&mut state, KeyCode::Char('j'), 18);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        let mut next_theme = theme();
        next_theme.syntax_theme_name = "TokyoNight".into();
        let next_layout = ChangeLayout::Interleaved {
            group: std::num::NonZeroUsize::new(1).unwrap(),
        };
        state.assemble(
            Column::Unstaged,
            8,
            next_layout,
            &next_theme,
            DrawBudget::Fast,
        );
        assert_eq!(state.unstaged.cursor_anchor(), Some(&anchor));
        wait_for_render(&mut state.unstaged);
        state.assemble(
            Column::Unstaged,
            8,
            next_layout,
            &next_theme,
            DrawBudget::Fast,
        );
        assert_eq!(state.unstaged.cursor_anchor(), Some(&anchor));
    }

    #[test]
    fn directory_reload_keeps_the_cursor_when_another_file_finishes_first() {
        let mut state = make_state(&[edited("src/a.rs"), large_file("src/b.rs")]);
        state.sidebar.set_selected(0, 18);
        state.visible_diff_window(DrawBudget::Full);
        let last = state
            .unstaged
            .rows()
            .iter()
            .rposition(|row| row.is_selectable())
            .unwrap();
        state.unstaged.select_row(last);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        state.unstaged.carry_over(&Default::default());
        state.visible_diff_window(DrawBudget::Fast);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !state.unstaged.cache.contains(grouped_epoch(80), 0) {
            state.background(true);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        state.visible_diff_window(DrawBudget::Fast);
        assert_eq!(state.unstaged.cursor.file(), Some("src/b.rs"));
        state.visible_diff_window(DrawBudget::Full);
        assert_eq!(state.unstaged.cursor_anchor(), Some(&anchor));
    }

    #[test]
    #[ignore = "release performance probe; run with --release --ignored --nocapture"]
    fn navigation_frame_costs() {
        let files: Vec<_> = (0..100)
            .map(|index| ResolvedFile {
                file: file_diff(&format!("src/f{index:03}.rs")),
                before: String::new(),
                after: (0..300)
                    .map(|line| format!("fn item_{line}() {{ let value = \"syntax\"; }}\n"))
                    .collect(),
            })
            .collect();
        let mut state = make_state(&files);
        state.sidebar.set_selected(0, 18);
        let start = Instant::now();
        state.visible_diff_window(DrawBudget::Fast);
        let pending = start.elapsed();
        let render_start = Instant::now();
        state.visible_diff_window(DrawBudget::Full);
        let ready = render_start.elapsed();
        let assemble = |state: &mut FilesMode| {
            state.assemble(
                Column::Unstaged,
                80,
                ChangeLayout::Grouped,
                &Theme::default(),
                DrawBudget::Fast,
            );
        };
        state.unstaged.snap_to_top();
        let assemble_start = Instant::now();
        assemble(&mut state);
        let assembly = assemble_start.elapsed();
        let retained_start = Instant::now();
        assemble(&mut state);
        println!(
            "directory: pending={pending:?}, complete={ready:?}, assembly={assembly:?}, retained={:?}, rows={}",
            retained_start.elapsed(),
            state.unstaged.window_rows()
        );
        assert!(
            assembly < Duration::from_millis(33),
            "directory assembly exceeded frame target: {assembly:?}"
        );
    }

    fn grouped_epoch(width: usize) -> CacheEpoch {
        CacheEpoch {
            width,
            layout: ChangeLayout::Grouped,
            // Match the theme the render path keys on (test_support `theme()`).
            syntax_theme: deltoids::theme_name_key(&theme().syntax_theme_name),
        }
    }

    /// A file whose diff has context, a removed line, and an added line at
    /// known numbers: `line one`, `line two`, `const x = 1;` → `2;`.
    fn edited(path: &str) -> ResolvedFile {
        ResolvedFile {
            file: file_diff(path),
            before: "line one\nline two\nconst x = 1;\nline four\n".to_string(),
            after: "line one\nline two\nconst x = 2;\nline four\n".to_string(),
        }
    }

    /// A Files mode focused on the diff pane with its window assembled,
    /// as the first draw would leave it.
    fn diff_state(files: &[ResolvedFile]) -> FilesMode {
        let mut state = make_state_with_rects(files);
        state.focus = Focus::Diff;
        rebuild_window(&mut state);
        state
    }

    /// Re-assemble the diff window, as a draw does.
    fn rebuild_window(state: &mut FilesMode) {
        let width = state.unstaged.cached_width;
        state.assemble(
            Column::Unstaged,
            width,
            ChangeLayout::Grouped,
            &Theme::default(),
            DrawBudget::Full,
        );
        wait_for_render(&mut state.unstaged);
        state.assemble(
            Column::Unstaged,
            width,
            ChangeLayout::Grouped,
            &Theme::default(),
            DrawBudget::Full,
        );
    }

    /// Save `text` at `anchor` through the same path the editor uses,
    /// then re-assemble as the following draw would.
    fn note(state: &mut FilesMode, anchor: &CommentAnchor, text: &str) {
        save_comment(state, anchor.clone(), text.to_string());
        rebuild_window(state);
    }

    fn cursor_line(state: &FilesMode) -> Option<(String, LineSide, usize)> {
        state
            .unstaged
            .cursor_anchor()
            .map(|anchor| (anchor.path.clone(), anchor.side, anchor.line))
    }

    fn window_text(state: &FilesMode) -> Vec<String> {
        state
            .unstaged
            .rows()
            .iter()
            .map(|row| line_text(&row.line))
            .collect()
    }

    /// Type `text` into an open comment editor.
    fn type_text(state: &mut FilesMode, text: &str) {
        for ch in text.chars() {
            handle_key(state, KeyCode::Char(ch), 18);
        }
    }

    #[test]
    fn diff_rows_anchor_each_line_to_its_file_and_number() {
        let state = diff_state(&[edited("src/app.rs")]);
        let anchors: Vec<(String, LineSide, usize)> = state
            .unstaged
            .rows()
            .iter()
            .filter_map(|row| row.anchor.as_ref())
            .map(|a| (a.path.clone(), a.side, a.line))
            .collect();

        // Context lines 1-2 (new-file numbering), the removed line at old
        // line 3, and the added line that replaces it at new line 3.
        assert_eq!(
            anchors,
            vec![
                ("src/app.rs".to_string(), LineSide::New, 1),
                ("src/app.rs".to_string(), LineSide::New, 2),
                ("src/app.rs".to_string(), LineSide::Old, 3),
                ("src/app.rs".to_string(), LineSide::New, 3),
            ]
        );
    }

    #[test]
    fn the_cursor_walks_diff_lines_across_the_files_in_the_window() {
        // Selecting the `src/` directory row puts both files in one window.
        let mut state = diff_state(&[edited("src/a.txt"), edited("src/b.txt")]);
        state.sidebar.set_selected(0, 18);
        rebuild_window(&mut state);

        let mut visited = Vec::new();
        for _ in 0..12 {
            if let Some(line) = cursor_line(&state) {
                visited.push(line);
            }
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }

        assert!(
            visited.iter().any(|(path, _, _)| path == "src/a.txt"),
            "the cursor starts in the first file: {visited:?}"
        );
        assert!(
            visited.iter().any(|(path, _, _)| path == "src/b.txt"),
            "and walks on into the second: {visited:?}"
        );
    }

    #[test]
    fn c_opens_the_editor_and_enter_saves_the_comment() {
        let mut state = diff_state(&[edited("app.rs")]);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();

        handle_key(&mut state, KeyCode::Char('c'), 18);
        assert!(matches!(state.input, InputState::Commenting { .. }));
        type_text(&mut state, "needs review");
        handle_key(&mut state, KeyCode::Enter, 18);

        assert!(matches!(state.input, InputState::Normal));
        assert_eq!(state.comments.note(&anchor), Some("needs review"));
        // The line it annotates is snapshotted with it.
        assert_eq!(
            state.comments.get(&anchor).map(|c| c.code.as_str()),
            Some("line one")
        );
    }

    #[test]
    fn c_does_nothing_off_a_diff_line() {
        let mut state = diff_state(&[edited("app.rs")]);
        state.focus = Focus::Sidebar;
        handle_key(&mut state, KeyCode::Char('c'), 18);
        assert!(matches!(state.input, InputState::Normal));

        // Focused on the diff but parked on the file header.
        state.focus = Focus::Diff;
        state.unstaged.cursor.row = 0;
        handle_key(&mut state, KeyCode::Char('c'), 18);
        assert!(matches!(state.input, InputState::Normal));
    }

    #[test]
    fn esc_cancels_without_changing_the_saved_comment() {
        let mut state = diff_state(&[edited("app.rs")]);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        note(&mut state, &anchor, "keep me");

        handle_key(&mut state, KeyCode::Char('c'), 18);
        type_text(&mut state, "x");
        handle_key(&mut state, KeyCode::Esc, 18);

        assert!(matches!(state.input, InputState::Normal));
        assert_eq!(state.comments.note(&anchor), Some("keep me"));
    }

    #[test]
    fn saving_empty_text_and_d_both_delete_the_comment() {
        let mut state = diff_state(&[edited("app.rs")]);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();

        note(&mut state, &anchor, "first pass");
        handle_key(&mut state, KeyCode::Char('c'), 18);
        for _ in 0.."first pass".len() {
            handle_key(&mut state, KeyCode::Backspace, 18);
        }
        handle_key(&mut state, KeyCode::Enter, 18);
        assert_eq!(state.comments.note(&anchor), None);

        note(&mut state, &anchor, "second pass");
        handle_key(&mut state, KeyCode::Char('d'), 18);
        assert_eq!(state.comments.note(&anchor), None);
    }

    #[test]
    fn saved_comments_render_under_their_diff_line() {
        let mut state = diff_state(&[edited("app.rs")]);
        // Comment on the added line (new line 3).
        for _ in 0..3 {
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        assert_eq!((anchor.side, anchor.line), (LineSide::New, 3));
        note(&mut state, &anchor, "explain this");

        let rows = state.unstaged.rows();
        let line_row = rows
            .iter()
            .position(|row| row.anchor.as_ref() == Some(&anchor))
            .expect("the commented line is still rendered");
        assert!(
            line_text(&rows[line_row + 1].line).contains("explain this"),
            "the comment renders directly under its line: {:#?}",
            window_text(&state)
        );
        assert!(
            !rows[line_row + 1].is_selectable(),
            "comment rows are not cursor stops"
        );
    }

    #[test]
    fn a_comment_never_drops_the_retained_render() {
        // A comment is drawn over the retained render, not into it.
        // Rebuilding for a comment would re-run syntax highlighting and
        // flash the "Rendering…" placeholder on the next frame.
        let mut state = diff_state(&[edited("src/a.txt"), edited("src/b.txt")]);
        // Selecting `src/` warms both files' blocks.
        state.sidebar.set_selected(0, 18);
        rebuild_window(&mut state);
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 0));
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 1));

        let anchor = CommentAnchor {
            path: "src/b.txt".to_string(),
            side: LineSide::New,
            line: 1,
        };
        note(&mut state, &anchor, "look here");

        assert!(state.unstaged.cache.contains(grouped_epoch(80), 0));
        assert!(
            state.unstaged.cache.contains(grouped_epoch(80), 1),
            "the commented file's render survives"
        );
        assert!(
            window_text(&state)
                .iter()
                .any(|row| row.contains("look here")),
            "and the comment is on screen"
        );
    }

    #[test]
    fn y_copies_every_comment_in_sidebar_order() {
        let mut state = diff_state(&[edited("src/a.txt"), edited("src/b.txt")]);
        state.sidebar.set_selected(0, 18);
        rebuild_window(&mut state);

        // Nothing to copy yet.
        let command = handle_key(&mut state, KeyCode::Char('y'), 18);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to copy"));

        let in_b = CommentAnchor {
            path: "src/b.txt".to_string(),
            side: LineSide::New,
            line: 3,
        };
        let in_a = CommentAnchor {
            path: "src/a.txt".to_string(),
            side: LineSide::Old,
            line: 3,
        };
        note(&mut state, &in_b, "second");
        note(&mut state, &in_a, "first");

        let command = handle_key(&mut state, KeyCode::Char('y'), 18);
        let text = match command {
            AppCommand::CopyToClipboard(text) => text,
            other => panic!("expected a clipboard copy, got {other:?}"),
        };
        assert_eq!(state.status.as_deref(), Some("Copied 2 comments"));
        // Repo-relative paths, correct sides, sidebar order.
        assert!(
            text.contains("src/a.txt:3\n- const x = 1;\nfirst\n"),
            "text was:\n{text}"
        );
        assert!(
            text.contains("src/b.txt:3\n+ const x = 2;\nsecond\n"),
            "text was:\n{text}"
        );
        assert!(text.find("\nfirst\n").unwrap() < text.find("\nsecond\n").unwrap());
    }

    #[test]
    fn shift_d_clears_every_comment_and_leaves_the_cache_untouched() {
        let mut state = diff_state(&[edited("src/a.txt"), edited("src/b.txt")]);
        state.sidebar.set_selected(0, 18);
        rebuild_window(&mut state);

        // Nothing to clear yet.
        let command = handle_key(&mut state, KeyCode::Char('D'), 18);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to clear"));

        let anchor = CommentAnchor {
            path: "src/b.txt".to_string(),
            side: LineSide::New,
            line: 1,
        };
        note(&mut state, &anchor, "look here");
        assert!(
            window_text(&state)
                .iter()
                .any(|row| row.contains("look here"))
        );

        let command = handle_key(&mut state, KeyCode::Char('D'), 18);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("Cleared 1 comment"));
        assert!(state.comments.is_empty());
        // Comments are an overlay: clearing must not evict the diff cache.
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 0));
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 1));
        // The note is gone from screen after the overlay re-renders.
        rebuild_window(&mut state);
        assert!(
            !window_text(&state)
                .iter()
                .any(|row| row.contains("look here")),
            "the cleared note must be off screen"
        );

        // A following copy has nothing to emit.
        let command = handle_key(&mut state, KeyCode::Char('y'), 18);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to copy"));
    }

    #[test]
    fn a_failed_copy_is_reported_instead_of_success() {
        let mut state = diff_state(&[edited("app.rs")]);
        state.status = Some("Copied 1 comment".to_string());

        Mode::report_copy(&mut state, Err("no clipboard".to_string()));
        assert_eq!(state.status.as_deref(), Some("Copy failed: no clipboard"));

        Mode::report_copy(&mut state, Ok(()));
        assert_eq!(
            state.status.as_deref(),
            Some("Copy failed: no clipboard"),
            "a successful copy leaves the mode's own message in place"
        );
    }

    #[test]
    fn the_open_editor_captures_keys_that_are_normally_bindings() {
        let mut state = diff_state(&[edited("app.rs")]);
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();

        assert!(!Mode::captures_text_input(&state));
        Mode::handle_key(&mut state, KeyCode::Char('c'), 18);
        assert!(
            Mode::captures_text_input(&state),
            "the shell must hand every key to the open editor"
        );

        for ch in "q?[]<>d".chars() {
            Mode::handle_key(&mut state, KeyCode::Char(ch), 18);
        }
        Mode::handle_key(&mut state, KeyCode::Enter, 18);

        assert!(!Mode::captures_text_input(&state));
        assert_eq!(state.comments.note(&anchor), Some("q?[]<>d"));
    }

    #[test]
    fn clicking_a_diff_line_moves_the_cursor_there() {
        let mut state = diff_state(&[edited("app.rs")]);
        let last_line_row = state
            .unstaged
            .rows()
            .iter()
            .rposition(|row| row.is_selectable())
            .unwrap();

        // Row 0 of the pane body is one below the pane's top border.
        let mouse = make_mouse(
            MouseEventKind::Down(MouseButton::Left),
            50,
            (last_line_row + 1) as u16,
        );
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.unstaged.cursor.row, last_line_row);

        // A click on the file header only focuses the pane.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 1);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.unstaged.cursor.row, last_line_row);
    }

    #[test]
    fn a_wrapped_diff_line_still_takes_one_cursor_stop() {
        let mut state = diff_state(&[edited("app.rs")]);
        // A pane far narrower than the content wraps every diff line.
        state.unstaged.cached_width = 8;
        state.unstaged.cache.clear();
        rebuild_window(&mut state);

        let rows = state.unstaged.rows();
        let selectable = rows.iter().filter(|row| row.is_selectable()).count();
        assert_eq!(selectable, 4, "one stop per diff line, not per row");
        assert!(
            rows.len() > selectable + 3,
            "the narrow pane should have produced wrapped rows"
        );
    }

    #[test]
    fn comments_follow_their_line_when_the_working_tree_shifts() {
        let mut state = diff_state(&[edited("app.rs")]);
        // Comment on the added line, `const x = 2;` at new line 3.
        for _ in 0..3 {
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        note(&mut state, &anchor, "look here");

        // Two lines are inserted above it, so the same text is now line 5.
        let shifted = ResolvedFile {
            file: file_diff("app.rs"),
            before: "line one\nline two\nconst x = 1;\nline four\n".to_string(),
            after: "line zero\nline half\nline one\nline two\nconst x = 2;\nline four\n"
                .to_string(),
        };
        let model = model_from(&[shifted]);
        let sections = model.sections(&state.display_order);
        reanchor(&mut state.comments, &sections);
        state.model = model;
        state.unstaged.cache.clear();
        rebuild_window(&mut state);

        let moved = CommentAnchor {
            line: 5,
            ..anchor.clone()
        };
        assert_eq!(state.comments.note(&anchor), None);
        assert_eq!(state.comments.note(&moved), Some("look here"));
        // It still renders under its line, and is not outdated.
        let text = window_text(&state).join("\n");
        assert!(text.contains("look here"), "rendered as:\n{text}");
        assert!(!text.contains("outdated"), "rendered as:\n{text}");
    }

    #[test]
    fn a_comment_whose_line_changed_under_it_renders_outdated() {
        let mut state = diff_state(&[edited("app.rs")]);
        for _ in 0..3 {
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }
        let anchor = state.unstaged.cursor_anchor().cloned().unwrap();
        note(&mut state, &anchor, "look here");

        // The commented line is rewritten in place: same number, new text,
        // and nothing else matches the snapshot, so it cannot be followed.
        state.model = model_from(&[ResolvedFile {
            file: file_diff("app.rs"),
            before: "line one\nline two\nconst x = 1;\nline four\n".to_string(),
            after: "line one\nline two\nconst x = 99;\nline four\n".to_string(),
        }]);
        let sections = state.model.sections(&state.display_order);
        reanchor(&mut state.comments, &sections);
        state.unstaged.cache.clear();
        rebuild_window(&mut state);

        let text = window_text(&state).join("\n");
        assert!(text.contains("look here"), "the note is kept:\n{text}");
        assert!(text.contains("outdated"), "and marked outdated:\n{text}");
    }

    #[test]
    fn a_reload_keeps_the_cursor_on_its_diff_line() {
        let mut state = diff_state(&[edited("app.rs")]);
        for _ in 0..2 {
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }
        let before = state.unstaged.cursor_anchor().cloned().unwrap();

        // A reload rebuilds every row from scratch.
        state.unstaged.carry_over(&Default::default());
        rebuild_window(&mut state);

        assert_eq!(state.unstaged.cursor_anchor(), Some(&before));
    }

    #[test]
    fn the_open_editor_shows_the_target_line_and_typed_text() {
        let mut state = diff_state(&[edited("src/app.rs")]);
        for _ in 0..2 {
            handle_key(&mut state, KeyCode::Char('j'), 18);
        }
        handle_key(&mut state, KeyCode::Char('c'), 18);
        type_text(&mut state, "look here");

        let screen = drawn_screen(&mut state);
        assert!(screen.contains("src/app.rs:3"), "screen was:\n{screen}");
        assert!(screen.contains("look here"));
        assert!(screen.contains("Enter save"));
    }

    #[test]
    fn the_diff_pane_footer_shows_the_copy_status() {
        let mut state = diff_state(&[edited("app.rs")]);
        handle_key(&mut state, KeyCode::Char('y'), 18);
        assert!(drawn_screen(&mut state).contains("No comments to copy"));
    }

    /// Build a model from resolved files, as `make_state` does.
    /// Draw one frame of Files mode and return the flattened screen text.
    fn drawn_screen(mode: &mut FilesMode) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::{Constraint, Direction, Layout};

        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        mode.assemble(
            Column::Unstaged,
            67,
            ChangeLayout::Grouped,
            &theme,
            DrawBudget::Fast,
        );
        wait_for_render(&mut mode.unstaged);
        term.draw(|frame| {
            let area = frame.area();
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(30), Constraint::Min(10)])
                .split(area);
            mode.draw(
                frame,
                cols[0],
                cols[1],
                ChangeLayout::Grouped,
                &theme,
                DrawBudget::Full,
            );
        })
        .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn error_mode_draws_message_not_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::{Constraint, Direction, Layout};

        let theme = Theme::default();
        let mut mode = FilesMode::error(
            &theme,
            80,
            "missing index blob deadbeef\nhint: try again".to_string(),
        );
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| {
            let area = f.area();
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(28), Constraint::Min(10)])
                .split(area);
            mode.draw(
                f,
                cols[0],
                cols[1],
                ChangeLayout::Grouped,
                &theme,
                DrawBudget::Full,
            );
        })
        .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("missing index blob"),
            "error message missing: {text:?}"
        );
        assert!(
            text.contains("hint: try again"),
            "hint line missing: {text:?}"
        );
        assert!(
            !text.contains("No local changes."),
            "error state must not show the clean message: {text:?}"
        );
    }

    #[test]
    fn handle_key_tab_toggles_focus() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Tab, 4);
        assert_eq!(state.focus, Focus::Diff);
        handle_key(&mut state, KeyCode::Tab, 4);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn pane_at_returns_correct_focus() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state_with_rects(&resolved);
        assert_eq!(pane_at(&state, 5, 5), Some(Focus::Sidebar));
        assert_eq!(pane_at(&state, 50, 5), Some(Focus::Diff));
        assert_eq!(pane_at(&state, 200, 200), None);
    }

    #[test]
    fn click_focuses_pane() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 5);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.focus, Focus::Diff);

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 5);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn click_outside_panes_is_noop() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Sidebar;
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 200, 200);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn selected_path_none_without_repo() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state(&resolved);
        // Without a repo there is no on-disk path.
        assert_eq!(Mode::selected_path(&state), None);
    }

    #[test]
    fn selected_path_joins_workdir_for_selected_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(&input, Some(&wrapper), std::collections::HashMap::new()).unwrap();
        let expected = wrapper.workdir().unwrap().join("a.txt");
        let state = FilesMode::new(
            model,
            Patches::net(&input),
            Some(wrapper),
            &Theme::default(),
            80,
        );

        assert_eq!(Mode::selected_path(&state), Some(expected));
    }

    /// The reload viewport used by the startup self-heal tests.
    fn reload_vp() -> Viewport {
        Viewport {
            height: 20,
            diff_width: 80,
        }
    }

    #[test]
    fn staging_only_reload_updates_the_view_once() {
        let dir = tempfile::tempdir().unwrap();
        let raw = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        stage_all(&raw);
        commit_index(&raw, "initial");
        std::fs::write(dir.path().join("a.txt"), "after\n").unwrap();
        raw.blob(b"after\n").unwrap();
        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let (input, model) = FilesMode::try_model(&wrapper).unwrap();
        let theme = theme();
        let mut mode = FilesMode::new(model, input.clone(), Some(wrapper), &theme, 80);
        let selected = mode.selected_path();
        assert!(!mode.reload(reload_vp(), &theme).unwrap());
        stage_all(&raw);
        assert!(mode.reload(reload_vp(), &theme).unwrap());
        assert_eq!(mode.last_input, input);
        assert_eq!(mode.selected_path(), selected);
        let status = mode.model.stages.get("a.txt").unwrap();
        assert_eq!(status.staged, Some(crate::sidebar::ChangeKind::Modified));
        assert_eq!(status.unstaged, None);
        assert!(!mode.reload(reload_vp(), &theme).unwrap());
    }

    #[test]
    fn failed_snapshot_retains_live_view_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let raw = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        stage_all(&raw);
        commit_index(&raw, "initial");
        std::fs::write(dir.path().join("a.txt"), "after\n").unwrap();
        raw.blob(b"after\n").unwrap();
        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let (input, model) = FilesMode::try_model(&wrapper).unwrap();
        let stages = model.stages.clone();
        let theme = theme();
        let mut mode = FilesMode::new(model, input.clone(), Some(wrapper), &theme, 80);
        let reference = raw.head().unwrap().name().unwrap().to_string();
        let path = raw.path().join(reference);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, "not-an-object-id\n").unwrap();
        assert!(!mode.reload(reload_vp(), &theme).unwrap());
        assert_eq!(mode.last_input, input);
        assert_eq!(mode.model.stages, stages);
        assert!(mode.retry_reload());
        std::fs::write(path, original).unwrap();
        assert!(!mode.reload(reload_vp(), &theme).unwrap());
        assert!(!mode.retry_reload());
    }

    #[test]
    fn snapshot_failure_at_startup_recovers_from_loading() {
        let dir = tempfile::tempdir().unwrap();
        let raw = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        stage_all(&raw);
        commit_index(&raw, "initial");
        let path = raw.path().join(raw.head().unwrap().name().unwrap());
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, "not-an-object-id\n").unwrap();
        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        assert!(FilesMode::try_model(&wrapper).is_err());
        let theme = theme();
        let mut mode = FilesMode::loading(wrapper, &theme, 80);
        assert!(!mode.reload(reload_vp(), &theme).unwrap());
        assert!(mode.startup_pending);
        std::fs::write(path, original).unwrap();
        assert!(mode.reload(reload_vp(), &theme).unwrap());
        assert!(!mode.startup_pending);
        assert!(!mode.retry_reload());
    }

    #[test]
    fn loading_mode_is_reloadable_and_self_heals_to_live_diff() {
        // A repo-backed startup that opened in Loading is non-static and
        // reloadable; the first successful reload promotes it to a live
        // diff and clears the loading banner. This is the deterministic
        // proof of the startup self-heal.
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let theme = Theme::default();
        let mut mode = FilesMode::loading(wrapper, &theme, 80);

        assert!(mode.repo.is_some(), "loading mode must be reloadable");
        assert!(mode.startup_pending, "loading mode is pending");
        assert!(mode.model.files.is_empty(), "loading mode has no files yet");

        let changed = mode.reload(reload_vp(), &theme).unwrap();
        assert!(changed, "first successful reload must rebuild");
        assert!(
            !mode.startup_pending,
            "a live diff clears the loading state"
        );
        assert_eq!(mode.model.files.len(), 1, "the diff is now live");
        assert_eq!(
            crate::sidebar::display_path(&mode.model.files[0].file),
            "a.txt"
        );
    }

    #[test]
    fn loading_mode_resolves_clean_tree_to_no_changes() {
        // A startup that opened Loading but whose tree is actually clean
        // resolves out of Loading on the first tick (an Unchanged outcome
        // at the empty sentinel confirms a clean tree).
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let theme = Theme::default();
        let mut mode = FilesMode::loading(wrapper, &theme, 80);

        let changed = mode.reload(reload_vp(), &theme).unwrap();
        assert!(changed, "leaving Loading changes the visible state");
        assert!(
            !mode.startup_pending,
            "a confirmed clean tree leaves Loading"
        );
        assert!(mode.repo.is_some(), "a clean tree is still watchable");
    }

    #[test]
    fn loading_mode_keeps_loading_on_failure_within_window() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let theme = Theme::default();
        let mut mode = FilesMode::loading(wrapper, &theme, 80);

        // A fresh failure (loading_since is now) must not degrade.
        let changed = mode.resolve_reload(
            ReloadOutcome::Failed("transient race".to_string()),
            &theme,
            80,
        );
        assert!(!changed);
        assert!(mode.startup_pending, "a fresh failure keeps Loading");
        assert!(mode.repo.is_some(), "a fresh failure stays reloadable");
    }

    #[test]
    fn loading_mode_degrades_to_error_after_window() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let theme = Theme::default();
        let mut mode = FilesMode::loading(wrapper, &theme, 80);

        // Pretend the loading window has already elapsed (the machine has
        // been up far longer than the timeout, so this never underflows).
        mode.loading_since = Some(Instant::now() - STARTUP_LOADING_TIMEOUT * 2);

        let changed = mode.resolve_reload(
            ReloadOutcome::Failed("missing index blob deadbeef".to_string()),
            &theme,
            80,
        );
        assert!(changed, "the error state requires a repaint");
        assert!(
            mode.repo.is_none(),
            "a failure that persists past the window degrades to a static error"
        );
        assert!(
            !mode.startup_pending,
            "the degraded state is no longer loading"
        );
    }
}
