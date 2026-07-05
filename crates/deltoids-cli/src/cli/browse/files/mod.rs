//! Files mode: the working-tree view of the unified TUI.
//!
//! Discovers the repository and shows its local working-tree changes
//! against `HEAD`. The working tree is watched and re-diffed on
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

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use deltoids::{Theme, git};

use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar::Sidebar;

use super::mode::{AppCommand, DrawBudget, Mode, ReloadViewport, TabStrip};

mod diff_pane;
mod model;
mod reload;
mod sidebar_pane;
#[cfg(test)]
mod test_support;

use diff_pane::{DiffPane, SCROLL_STEP_LARGE, SCROLL_STEP_SMALL};
use model::{DiffSource, Model, build_model};
use reload::{reload_working_tree, should_reload, spawn_watcher};
use sidebar_pane::build_sidebar;

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
            stages: Default::default(),
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
        let diff = DiffPane::new(display_order, width);
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

    /// Clear the per-file cache when the pane width changed; the next draw
    /// re-renders only the visible window on demand at the new width.
    fn ensure_width(&mut self, diff_width: usize) {
        if diff_width == 0 || diff_width == self.diff.cached_width {
            return;
        }
        self.diff.cache.clear();
        self.diff.cached_width = diff_width;
    }

    /// Assemble the diff window that should be visible right now.
    #[cfg(test)]
    fn visible_diff_window(&mut self, budget: DrawBudget) -> Vec<ratatui::text::Line<'static>> {
        let dr = self.sidebar.selection_display_range();
        let width = self.diff.cached_width;
        self.diff
            .assemble_window(dr, &self.model, width, &Theme::default(), budget)
    }

    /// Sync the diff pane's scroll to the top of the selected file's
    /// window (a sidebar move re-derives the window).
    fn snap_diff_to_selected_file(&mut self) {
        self.diff.snap_to_top();
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
            state
                .diff
                .scroll_by(SCROLL_STEP_LARGE as isize, diff_viewport);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state
                .diff
                .scroll_by(-(SCROLL_STEP_LARGE as isize), diff_viewport);
            AppCommand::Continue
        }
        // Remaining nav keys route to the focused pane. A sidebar move
        // also snaps the diff (cross-pane coordination owned here).
        other => {
            match state.focus {
                Focus::Sidebar => {
                    if sidebar_pane::handle_key(&mut state.sidebar, other, sidebar_viewport) {
                        state.snap_diff_to_selected_file();
                    }
                }
                Focus::Diff => {
                    state.diff.handle_key(other, diff_viewport);
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
    // Ctrl + wheel redirects the scroll to the sidebar list regardless
    // of hover position, so the diff can be scrolled by hovering it while
    // Ctrl steps through files. (Shift+wheel is swallowed by common
    // terminals/tmux as a mouse-mode bypass, so Ctrl is used instead.)
    let is_scroll = matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    );
    let modified = mouse.modifiers.contains(KeyModifiers::CONTROL);
    let target = if is_scroll && modified {
        Focus::Sidebar
    } else {
        match pane_at(state, mouse.column, mouse.row) {
            Some(pane) => pane,
            None => return AppCommand::Continue,
        }
    };

    match mouse.kind {
        MouseEventKind::ScrollDown => match target {
            Focus::Sidebar => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_down(sidebar_viewport);
                    state.snap_diff_to_selected_file();
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::Content);
                state
                    .diff
                    .scroll_by((steps * SCROLL_STEP_SMALL) as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        MouseEventKind::ScrollUp => match target {
            Focus::Sidebar => {
                let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_up(sidebar_viewport);
                    state.snap_diff_to_selected_file();
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Up, ScrollKind::Content);
                state
                    .diff
                    .scroll_by(-((steps * SCROLL_STEP_SMALL) as isize), diff_viewport);
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
                    state.snap_diff_to_selected_file();
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
        budget: DrawBudget,
    ) {
        let diff_width = right.width.saturating_sub(2) as usize;
        self.ensure_width(diff_width);

        self.sidebar_rect = left;
        self.diff_rect = right;

        let dr = self.sidebar.selection_display_range();
        let sidebar_focused = self.focus == Focus::Sidebar;
        let border = deltoids::render_tui::pane_border_color(sidebar_focused, theme);
        sidebar_pane::draw_sidebar(
            frame,
            left,
            &self.sidebar,
            &self.diff.display_order,
            sidebar_focused,
            tabs.title_line(border, theme),
            theme,
        );
        let window = self
            .diff
            .assemble_window(dr, &self.model, diff_width, theme, budget);
        self.diff
            .render(frame, right, self.focus == Focus::Diff, theme, window);
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

    fn selected_path(&self) -> Option<PathBuf> {
        // The selected file's workdir-relative path, joined onto the
        // repo's working directory. `None` for a piped/static source (no
        // repo on disk) or when there is no file under the selection.
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
    fn selected_path_none_without_repo() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state(&resolved);
        // Static/piped source (no repo on disk) has no on-disk path.
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
        let model = build_model(&input, Some(&wrapper)).unwrap();
        let expected = wrapper.workdir().unwrap().join("a.txt");
        let state = FilesMode::new(model, input, Some(wrapper), false, &Theme::default(), 80);

        assert_eq!(Mode::selected_path(&state), Some(expected));
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
