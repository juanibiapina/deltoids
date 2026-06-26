//! Files mode: the working-tree view of the unified TUI.
//!
//! Discovers the repository and shows its local working-tree changes
//! against `HEAD`. Live: the working tree is watched and re-diffed on
//! change. Outside a repo the mode degrades to an empty
//! "No local changes." state instead of erroring, so the TUI still opens.
//!
//! Layout: a file-tree sidebar (left column) and the deltoids diff
//! renderer (right pane). Selecting a file scrolls the diff to it.
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
//! - [`sidebar_pane`]: the sidebar's build, keys, render, footer.
//! - [`reload`]: the working-tree watcher and in-place rebuild.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use deltoids::{Theme, git};

use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar::Sidebar;

use super::mode::{AppCommand, Mode, ReloadViewport, TabStrip};

mod diff_pane;
mod model;
mod reload;
mod sidebar_pane;
#[cfg(test)]
mod test_support;

use diff_pane::{DiffPane, SCROLL_STEP_LARGE, SCROLL_STEP_SMALL, build_view};
use model::{DiffSource, Model, build_model};
use reload::{reload_working_tree, should_reload, spawn_watcher};
use sidebar_pane::build_sidebar;

/// Minimum wall-clock gap between full diff-cache rebuilds. Rebuilding
/// the whole cache on every resize step stalls the loop; between rebuilds
/// the diff is drawn from the existing cache (ratatui clips it) and snaps
/// correct once resizing settles within this interval.
const DIFF_REBUILD_THROTTLE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Sidebar,
    Diff,
}

/// Whether the discovered repo has any local working-tree changes
/// against `HEAD`. False outside a repo or on any git error. Cheap: it
/// runs `working_tree_diff` but skips the expensive model build, so it
/// can drive the smart starting-mode choice without paying Files mode's
/// full startup cost.
pub(in crate::cli::browse) fn working_tree_has_changes() -> bool {
    git::Repo::discover().is_some_and(|repo| repo_has_changes(&repo))
}

fn repo_has_changes(repo: &git::Repo) -> bool {
    repo.working_tree_diff()
        .ok()
        .is_some_and(|diff| !diff.trim().is_empty())
}

/// Files-mode state plus the data it renders. Owns the model, the repo
/// (for blob resolution and reload), and the reload bookkeeping; the
/// shell owns sidebar width, focus across modes, help, and the divider.
pub(super) struct FilesMode {
    /// Diff pane vertical slice.
    diff: DiffPane,
    /// Sidebar pane state: rows, selection, scroll.
    sidebar: Sidebar,
    /// Currently-focused pane within this mode.
    focus: Focus,
    /// Last-drawn pane rects, used for mouse hit-testing.
    sidebar_rect: Rect,
    diff_rect: Rect,
    /// Translates fanned-out mouse-wheel events into proportional motion.
    wheel: WheelScroll<Focus>,
    /// The owned data: resolved files plus their diffs.
    model: Model,
    /// The repo (for blob resolution and working-tree reload), if any.
    repo: Option<git::Repo>,
    /// True for a piped/empty source that never refreshes.
    is_static: bool,
    /// The diff text the current model was built from; the poll compares
    /// fresh `working_tree_diff` output against this to skip rebuilds.
    last_input: String,
    /// Throttles full diff-cache rebuilds during a resize.
    last_rebuild: Instant,
    /// Keeps the filesystem watcher alive for the session; dropping it
    /// closes the change channel.
    _watcher: Option<notify::RecommendedWatcher>,
}

impl FilesMode {
    /// Build the mode from the discovered repo's working tree, or an empty
    /// state when not in a repo. `initial_diff_width` seeds the diff cache
    /// for the first frame.
    pub(super) fn build(theme: &Theme, initial_diff_width: usize) -> Result<Self, String> {
        let (input, repo, is_static) = match git::Repo::discover() {
            Some(repo) => {
                let input = repo.working_tree_diff()?;
                (input, Some(repo), false)
            }
            // Not a repo: degrade to the empty state instead of erroring,
            // so the TUI still opens.
            None => (String::new(), None, true),
        };
        let model = build_model(&input, repo.as_ref())?;
        Ok(Self::new(
            model,
            input,
            repo,
            is_static,
            theme,
            initial_diff_width,
        ))
    }

    /// A cheap empty Files mode: no repo, no diff, static. Used as the
    /// startup placeholder for the inactive mode and as a degraded
    /// fallback when a real build fails.
    pub(super) fn empty(theme: &Theme, width: usize) -> Self {
        let model = Model {
            files: Vec::new(),
            diffs: Vec::new(),
        };
        Self::new(model, String::new(), None, true, theme, width)
    }

    fn new(
        model: Model,
        input: String,
        repo: Option<git::Repo>,
        is_static: bool,
        theme: &Theme,
        width: usize,
    ) -> Self {
        let sidebar = build_sidebar(&model, theme);
        let display_order = sidebar.display_order();
        let view = build_view(&model.files, &model.diffs, &display_order, width, theme);
        let diff = DiffPane::new(view, display_order, width);
        Self {
            diff,
            sidebar,
            focus: Focus::Sidebar,
            sidebar_rect: Rect::default(),
            diff_rect: Rect::default(),
            wheel: WheelScroll::new(),
            model,
            repo,
            is_static,
            last_input: input,
            last_rebuild: Instant::now(),
            _watcher: None,
        }
    }

    /// The active [`DiffSource`] for reload/watch, derived from owned
    /// state. Borrows `self.repo`.
    fn source(&self) -> DiffSource<'_> {
        match (self.is_static, self.repo.as_ref()) {
            (false, Some(repo)) => DiffSource::WorkingTree(repo),
            _ => DiffSource::Static,
        }
    }

    /// Rebuild the diff line cache if the pane width changed, throttled so
    /// a held resize key doesn't stall the loop.
    fn ensure_width(&mut self, diff_width: usize, diff_viewport: usize, theme: &Theme) {
        if diff_width == 0 || diff_width == self.diff.cached_width {
            return;
        }
        if self.last_rebuild.elapsed() < DIFF_REBUILD_THROTTLE {
            return;
        }
        let view = build_view(
            &self.model.files,
            &self.model.diffs,
            &self.diff.display_order,
            diff_width,
            theme,
        );
        self.diff.diff_lines = view.lines;
        self.diff.file_offsets = view.file_offsets;
        self.diff.cached_width = diff_width;
        self.last_rebuild = Instant::now();
        let dr = self.sidebar.selection_display_range();
        let max = self.diff.max_scroll(dr, diff_viewport);
        if self.diff.diff_scroll > max {
            self.diff.diff_scroll = max;
        }
    }

    /// Window of the diff that should be visible right now.
    #[cfg(test)]
    fn visible_diff_range(&self) -> std::ops::Range<usize> {
        self.diff
            .visible_range(self.sidebar.selection_display_range())
    }

    /// Sync the diff pane's scroll to the file the sidebar points at.
    fn snap_diff_to_selected_file(&mut self, viewport: usize) {
        let dr = self.sidebar.selection_display_range();
        let Some(file_idx) = self.sidebar.nearest_file_index() else {
            return;
        };
        self.diff.snap_to_file(file_idx, viewport, dr);
    }
}

/// Handle a mode-internal key (the shell strips global bindings first).
fn handle_key(
    state: &mut FilesMode,
    key: KeyCode,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    match key {
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
            let dr = state.sidebar.selection_display_range();
            state
                .diff
                .scroll_by(SCROLL_STEP_LARGE as isize, diff_viewport, dr);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            let dr = state.sidebar.selection_display_range();
            state
                .diff
                .scroll_by(-(SCROLL_STEP_LARGE as isize), diff_viewport, dr);
            AppCommand::Continue
        }
        // Remaining nav keys route to the focused pane. A sidebar move
        // also snaps the diff (cross-pane coordination owned here).
        other => {
            match state.focus {
                Focus::Sidebar => {
                    if sidebar_pane::handle_key(&mut state.sidebar, other, sidebar_viewport) {
                        state.snap_diff_to_selected_file(diff_viewport);
                    }
                }
                Focus::Diff => {
                    let dr = state.sidebar.selection_display_range();
                    state.diff.handle_key(other, diff_viewport, dr);
                }
            }
            AppCommand::Continue
        }
    }
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

fn handle_mouse(
    state: &mut FilesMode,
    mouse: MouseEvent,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    let target = match pane_at(state, mouse.column, mouse.row) {
        Some(pane) => pane,
        None => return AppCommand::Continue,
    };

    match mouse.kind {
        MouseEventKind::ScrollDown => match target {
            Focus::Sidebar => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_down(sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::Content);
                let dr = state.sidebar.selection_display_range();
                state
                    .diff
                    .scroll_by((steps * SCROLL_STEP_SMALL) as isize, diff_viewport, dr);
                AppCommand::Continue
            }
        },
        MouseEventKind::ScrollUp => match target {
            Focus::Sidebar => {
                let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_up(sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Up, ScrollKind::Content);
                let dr = state.sidebar.selection_display_range();
                state
                    .diff
                    .scroll_by(-((steps * SCROLL_STEP_SMALL) as isize), diff_viewport, dr);
                AppCommand::Continue
            }
        },
        MouseEventKind::Down(MouseButton::Left) => {
            state.focus = target;
            if target == Focus::Sidebar {
                let rect = state.sidebar_rect;
                let content_y = mouse.row.saturating_sub(rect.y).saturating_sub(1) as usize;
                let clicked = state.sidebar.scroll() + content_y;
                if clicked < state.sidebar.row_count() {
                    state.sidebar.set_selected(clicked, sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
            }
            AppCommand::Continue
        }
        _ => AppCommand::Continue,
    }
}

impl Mode for FilesMode {
    fn draw(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        left: Rect,
        right: Rect,
        tabs: TabStrip,
        theme: &Theme,
    ) {
        let diff_width = right.width.saturating_sub(2) as usize;
        let diff_viewport = right.height.saturating_sub(2) as usize;
        self.ensure_width(diff_width, diff_viewport, theme);

        self.sidebar_rect = left;
        self.diff_rect = right;

        let dr = self.sidebar.selection_display_range();
        sidebar_pane::draw_sidebar(
            frame,
            left,
            &self.sidebar,
            &self.diff.display_order,
            self.focus == Focus::Sidebar,
            tabs.title_line(theme),
            theme,
        );
        self.diff
            .render(frame, right, self.focus == Focus::Diff, theme, dr);
    }

    fn handle_key(
        &mut self,
        key: KeyCode,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        handle_key(self, key, right_viewport, left_viewport)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        handle_mouse(self, mouse, right_viewport, left_viewport)
    }

    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>> {
        // Arm the watcher; a static source yields a receiver that never
        // fires. Keep the watcher handle in `self` so the channel stays
        // open for the session.
        let (watcher, rx) = spawn_watcher(&self.source()).ok()?;
        self._watcher = watcher;
        Some(rx)
    }

    fn should_reload(&self, paths: &[PathBuf]) -> bool {
        should_reload(&self.source(), paths)
    }

    fn needs_git_poll(&self) -> bool {
        matches!(self.source(), DiffSource::WorkingTree(_))
    }

    fn reload(&mut self, viewport: ReloadViewport, theme: &Theme) -> Result<bool, String> {
        if self.is_static {
            return Ok(false);
        }
        let Some(repo) = self.repo.as_ref() else {
            return Ok(false);
        };
        let width = if viewport.right_width > 0 {
            viewport.right_width
        } else {
            self.diff.cached_width
        };
        // `repo` borrows `self.repo`; the rest are disjoint fields.
        reload_working_tree(
            &mut self.diff,
            &mut self.sidebar,
            &mut self.model,
            &mut self.last_input,
            repo,
            theme,
            width,
            viewport.right_viewport,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::test_support::*;
    use model::ResolvedFile;

    #[test]
    fn handle_key_tab_toggles_focus() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Tab, 4, 4);
        assert_eq!(state.focus, Focus::Diff);
        handle_key(&mut state, KeyCode::Tab, 4, 4);
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
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.focus, Focus::Diff);

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 5);
        handle_mouse(&mut state, mouse, 18, 18);
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
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn repo_has_changes_detects_working_tree_edits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        assert!(!repo_has_changes(&wrapper), "clean tree has no changes");

        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        assert!(repo_has_changes(&wrapper), "edited tree has changes");
    }
}
