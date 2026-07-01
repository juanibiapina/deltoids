//! Traces mode: browse edit/write traces for the current directory.
//!
//! Layout (lazygit-inspired):
//! - Left column, top: entries (edits/writes) of the selected trace.
//! - Left column, bottom: traces for the current working directory.
//! - Right pane: diff / detail for the selected entry.
//!
//! Focus cycles entries → traces → diff with `Tab`.
//!
//! ## Module layout
//!
//! Split by change axis. This file is the mode adapter: it owns the
//! mode's state, its key/mouse handling, its render, and its live
//! reload, and implements [`super::mode::Mode`]. Each pane owns its
//! slice:
//!
//! - [`model`]: load traces/entries for the current directory.
//! - [`entries_pane`] / [`traces_pane`]: the two list slices.
//! - [`detail`]: the detail/diff slice (cache + renderers).
//! - [`reload`]: reload from disk, preserving selection.
//! - [`scripted`]: the headless (non-TTY) render path.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::Style,
    widgets::{ListState, Paragraph},
};

use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use deltoids::Theme;
use deltoids::render_tui::{
    pane_block, pane_block_with_footer, pane_border_color, position_footer, rgb_to_color,
};

use super::mode::{AppCommand, Mode, ReloadViewport, TabStrip};

mod detail;
mod entries_pane;
mod model;
mod reload;
mod scripted;
#[cfg(test)]
mod test_support;
mod traces_pane;

use detail::{DiffCache, max_detail_scroll, render_diff_pane};
use entries_pane::{move_entry_down, move_entry_up, render_entries_pane};
use model::{LoadedTrace, current_cwd_or_empty, load_traces_for_cwd};
use reload::reload_traces;
use scripted::run_scripted;
use traces_pane::{move_trace_down, move_trace_up, render_traces_pane};

const DIFF_SCROLL_STEP: usize = 3;
const DIFF_MOUSE_SCROLL_STEP: usize = 1;

/// Run the headless (non-TTY) scripted render path for the current
/// directory. Used by `deltoids traces` when stdout is not a terminal.
pub(in crate::cli::browse) fn run_scripted_for_cwd() -> Result<(), String> {
    let cwd = current_cwd_or_empty();
    let loaded = load_traces_for_cwd(&cwd)?;
    let theme = Theme::load();
    run_scripted(&loaded, &theme)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Traces,
    Entries,
    Diff,
}

#[derive(Debug, Clone)]
struct AppState {
    focus: Focus,
    trace_index: usize,
    entry_indices: Vec<usize>,
    diff_scroll: usize,
    diff_cache: Option<DiffCache>,
    entries_list_state: ListState,
    traces_list_state: ListState,
    /// Last-drawn pane rects, used for mouse hit-testing.
    entries_rect: Rect,
    traces_rect: Rect,
    diff_rect: Rect,
    /// Translates fanned-out mouse-wheel events into proportional motion.
    wheel: WheelScroll<Focus>,
}

impl AppState {
    fn new(trace_count: usize) -> Self {
        Self {
            focus: Focus::Entries,
            trace_index: 0,
            entry_indices: vec![0; trace_count],
            diff_scroll: 0,
            diff_cache: None,
            entries_list_state: ListState::default().with_selected(Some(0)),
            traces_list_state: ListState::default().with_selected(Some(0)),
            entries_rect: Rect::default(),
            traces_rect: Rect::default(),
            diff_rect: Rect::default(),
            wheel: WheelScroll::new(),
        }
    }

    fn entry_index(&self) -> usize {
        self.entry_indices
            .get(self.trace_index)
            .copied()
            .unwrap_or(0)
    }

    fn set_entry_index(&mut self, value: usize) {
        if let Some(slot) = self.entry_indices.get_mut(self.trace_index) {
            *slot = value;
        }
        self.entries_list_state.select(Some(value));
    }
}

/// Traces-mode state plus the loaded traces it renders.
pub(super) struct TracesMode {
    state: AppState,
    traces: Vec<LoadedTrace>,
    cwd: String,
    /// Keeps the trace-root watcher alive for the session.
    _watcher: Option<notify::RecommendedWatcher>,
}

impl TracesMode {
    /// Load the traces for the current directory and build the mode.
    pub(super) fn build() -> Result<Self, String> {
        let cwd = current_cwd_or_empty();
        let traces = load_traces_for_cwd(&cwd)?;
        let state = AppState::new(traces.len());
        Ok(Self {
            state,
            traces,
            cwd,
            _watcher: None,
        })
    }

    /// A cheap empty Traces mode (no traces loaded). Used as the startup
    /// placeholder for the inactive mode and as a degraded fallback.
    pub(super) fn empty() -> Self {
        Self {
            state: AppState::new(0),
            traces: Vec::new(),
            cwd: current_cwd_or_empty(),
            _watcher: None,
        }
    }

    /// Detail-row count from the cached render (0 when not yet built).
    fn detail_row_count(&self) -> usize {
        self.state
            .diff_cache
            .as_ref()
            .map(|cache| cache.lines.len())
            .unwrap_or(0)
    }
}

fn handle_key(
    state: &mut AppState,
    traces: &[LoadedTrace],
    key: KeyCode,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    match key {
        KeyCode::Tab => {
            state.focus = match state.focus {
                Focus::Entries => Focus::Traces,
                Focus::Traces => Focus::Diff,
                Focus::Diff => Focus::Entries,
            };
            AppCommand::Continue
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Entries => Focus::Diff,
                Focus::Traces => Focus::Entries,
                Focus::Diff => Focus::Traces,
            };
            AppCommand::Continue
        }
        KeyCode::Char('1') => {
            state.focus = Focus::Entries;
            AppCommand::Continue
        }
        KeyCode::Char('2') => {
            state.focus = Focus::Traces;
            AppCommand::Continue
        }
        KeyCode::Char('3') => {
            state.focus = Focus::Diff;
            AppCommand::Continue
        }
        KeyCode::Enter => {
            if state.focus == Focus::Traces {
                state.focus = Focus::Entries;
            }
            AppCommand::Continue
        }
        KeyCode::Char('J') => {
            let max_scroll = max_detail_scroll(detail_row_count, detail_height);
            state.diff_scroll = (state.diff_scroll + DIFF_SCROLL_STEP).min(max_scroll);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP);
            AppCommand::Continue
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if state.focus == Focus::Diff {
                let max_scroll = max_detail_scroll(detail_row_count, detail_height);
                state.diff_scroll = (state.diff_scroll + DIFF_SCROLL_STEP).min(max_scroll);
            } else {
                move_down(state, traces);
            }
            AppCommand::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.focus == Focus::Diff {
                state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP);
            } else {
                move_up(state, traces);
            }
            AppCommand::Continue
        }
        KeyCode::PageDown => {
            let max_scroll = max_detail_scroll(detail_row_count, detail_height);
            state.diff_scroll = (state.diff_scroll + detail_height.max(1)).min(max_scroll);
            AppCommand::Continue
        }
        KeyCode::PageUp => {
            state.diff_scroll = state.diff_scroll.saturating_sub(detail_height.max(1));
            AppCommand::Continue
        }
        _ => AppCommand::Continue,
    }
}

fn move_down(state: &mut AppState, traces: &[LoadedTrace]) {
    match state.focus {
        Focus::Traces => move_trace_down(state, traces),
        Focus::Entries => move_entry_down(state, traces),
        Focus::Diff => {}
    }
}

fn move_up(state: &mut AppState, _traces: &[LoadedTrace]) {
    match state.focus {
        Focus::Traces => move_trace_up(state),
        Focus::Entries => move_entry_up(state),
        Focus::Diff => {}
    }
}

fn pane_at(state: &AppState, col: u16, row: u16) -> Option<Focus> {
    let pos = Position::new(col, row);
    if state.entries_rect.contains(pos) {
        Some(Focus::Entries)
    } else if state.traces_rect.contains(pos) {
        Some(Focus::Traces)
    } else if state.diff_rect.contains(pos) {
        Some(Focus::Diff)
    } else {
        None
    }
}

fn handle_mouse(
    state: &mut AppState,
    traces: &[LoadedTrace],
    mouse: MouseEvent,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    let target = match pane_at(state, mouse.column, mouse.row) {
        Some(pane) => pane,
        None => return AppCommand::Continue,
    };

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll_down(state, traces, target, detail_row_count, detail_height)
        }
        MouseEventKind::ScrollUp => handle_mouse_scroll_up(state, target),
        MouseEventKind::Down(MouseButton::Left) => {
            handle_mouse_click(state, traces, target, mouse.row)
        }
        _ => AppCommand::Continue,
    }
}

fn handle_mouse_scroll_down(
    state: &mut AppState,
    traces: &[LoadedTrace],
    target: Focus,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    match target {
        Focus::Entries => {
            let steps = state
                .wheel
                .advance(target, ScrollDir::Down, ScrollKind::List);
            for _ in 0..steps {
                move_entry_down(state, traces);
            }
        }
        Focus::Traces => {
            let steps = state
                .wheel
                .advance(target, ScrollDir::Down, ScrollKind::List);
            for _ in 0..steps {
                move_trace_down(state, traces);
            }
        }
        Focus::Diff => {
            let steps = state
                .wheel
                .advance(target, ScrollDir::Down, ScrollKind::Content);
            let max_scroll = max_detail_scroll(detail_row_count, detail_height);
            state.diff_scroll =
                (state.diff_scroll + steps * DIFF_MOUSE_SCROLL_STEP).min(max_scroll);
        }
    }
    AppCommand::Continue
}

fn handle_mouse_scroll_up(state: &mut AppState, target: Focus) -> AppCommand {
    match target {
        Focus::Entries => {
            let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
            for _ in 0..steps {
                move_entry_up(state);
            }
        }
        Focus::Traces => {
            let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
            for _ in 0..steps {
                move_trace_up(state);
            }
        }
        Focus::Diff => {
            let steps = state
                .wheel
                .advance(target, ScrollDir::Up, ScrollKind::Content);
            state.diff_scroll = state
                .diff_scroll
                .saturating_sub(steps * DIFF_MOUSE_SCROLL_STEP);
        }
    }
    AppCommand::Continue
}

fn handle_mouse_click(
    state: &mut AppState,
    traces: &[LoadedTrace],
    target: Focus,
    row: u16,
) -> AppCommand {
    state.focus = target;

    match target {
        Focus::Entries => {
            let rect = state.entries_rect;
            let content_y = row.saturating_sub(rect.y).saturating_sub(1) as usize;
            let scroll_offset = state.entries_list_state.offset();
            let clicked = scroll_offset + content_y;
            let entry_count = traces
                .get(state.trace_index)
                .map(|t| t.entries.len())
                .unwrap_or(0);
            if clicked < entry_count {
                state.set_entry_index(clicked);
                state.diff_scroll = 0;
            }
        }
        Focus::Traces => {
            let rect = state.traces_rect;
            let content_y = row.saturating_sub(rect.y).saturating_sub(1) as usize;
            let scroll_offset = state.traces_list_state.offset();
            let clicked = scroll_offset + content_y;
            if clicked < traces.len() {
                state.trace_index = clicked;
                state.traces_list_state.select(Some(clicked));
                state.diff_scroll = 0;
            }
        }
        Focus::Diff => {}
    }

    AppCommand::Continue
}

impl Mode for TracesMode {
    fn draw(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        left: Rect,
        right: Rect,
        tabs: TabStrip,
        theme: &Theme,
    ) {
        let sidebar = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(left);

        let border = pane_border_color(self.state.focus == Focus::Entries, theme);
        let title = tabs.title_line(border, theme);

        if self.traces.is_empty() {
            render_empty(frame, &sidebar, right, title, border, theme);
            return;
        }

        self.state.entries_rect = sidebar[0];
        self.state.traces_rect = sidebar[1];
        self.state.diff_rect = right;

        let active_trace = &self.traces[self.state.trace_index];
        render_entries_pane(
            frame,
            sidebar[0],
            active_trace,
            &mut self.state,
            title,
            theme,
        );
        render_traces_pane(frame, sidebar[1], &self.traces, &mut self.state, theme);
        render_diff_pane(frame, right, active_trace, &mut self.state, theme);
    }

    fn handle_key(
        &mut self,
        key: KeyCode,
        _left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        let rows = self.detail_row_count();
        handle_key(&mut self.state, &self.traces, key, rows, right_viewport)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        let rows = self.detail_row_count();
        handle_mouse(&mut self.state, &self.traces, mouse, rows, right_viewport)
    }

    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>> {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let trace_root = crate::trace_root_directory().ok()?;
        std::fs::create_dir_all(&trace_root).ok()?;
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event.paths);
            }
        })
        .ok()?;
        watcher.watch(&trace_root, RecursiveMode::Recursive).ok()?;
        self._watcher = Some(watcher);
        Some(rx)
    }

    fn should_reload(&self, _paths: &[PathBuf]) -> bool {
        // Any change under the trace root warrants a reload; reload_traces
        // restores the selection and is cheap.
        true
    }

    fn needs_git_poll(&self) -> bool {
        false
    }

    fn reload(&mut self, _viewport: ReloadViewport, _theme: &Theme) -> Result<bool, String> {
        reload_traces(&mut self.traces, &mut self.state, &self.cwd)?;
        Ok(true)
    }
}

/// Render the empty-state panes (no traces for this directory) while
/// still drawing the tab strip so the user can toggle out of this mode.
fn render_empty(
    frame: &mut ratatui::Frame<'_>,
    sidebar: &[Rect],
    right: Rect,
    title: ratatui::text::Line<'static>,
    entries_border: ratatui::style::Color,
    theme: &Theme,
) {
    let message = Paragraph::new("No traces found for this directory.")
        .style(Style::default().fg(rgb_to_color(theme.muted)))
        .block(deltoids::render_tui::pane_block_with_tabs(
            title,
            entries_border,
            Some(position_footer(0, 0)),
        ));
    frame.render_widget(message, sidebar[0]);
    frame.render_widget(
        pane_block_with_footer(
            "─[2]─Traces─",
            rgb_to_color(theme.border),
            Some(position_footer(0, 0)),
        ),
        sidebar[1],
    );
    frame.render_widget(pane_block("─[3]─Diff─", rgb_to_color(theme.border)), right);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;

    #[test]
    fn tab_cycles_focus() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "Update x"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(state.focus, Focus::Entries);

        handle_key(&mut state, &traces, KeyCode::Tab, 0, 0);
        assert_eq!(state.focus, Focus::Traces);
        handle_key(&mut state, &traces, KeyCode::Tab, 0, 0);
        assert_eq!(state.focus, Focus::Diff);
        handle_key(&mut state, &traces, KeyCode::Tab, 0, 0);
        assert_eq!(state.focus, Focus::Entries);
        handle_key(&mut state, &traces, KeyCode::BackTab, 0, 0);
        assert_eq!(state.focus, Focus::Diff);
    }

    #[test]
    fn number_shortcuts_set_focus() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        handle_key(&mut state, &traces, KeyCode::Char('1'), 0, 0);
        assert_eq!(state.focus, Focus::Entries);
        handle_key(&mut state, &traces, KeyCode::Char('2'), 0, 0);
        assert_eq!(state.focus, Focus::Traces);
        handle_key(&mut state, &traces, KeyCode::Char('3'), 0, 0);
        assert_eq!(state.focus, Focus::Diff);
    }

    #[test]
    fn shift_jk_scrolls_diff_from_any_focus() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Entries;
        handle_key(&mut state, &traces, KeyCode::Char('J'), 20, 4);
        assert_eq!(state.diff_scroll, DIFF_SCROLL_STEP);
        state.focus = Focus::Traces;
        handle_key(&mut state, &traces, KeyCode::Char('K'), 20, 4);
        assert_eq!(state.diff_scroll, 0);
    }

    #[test]
    fn j_scrolls_diff_when_focused_on_diff() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Diff;
        handle_key(&mut state, &traces, KeyCode::Char('j'), 20, 4);
        assert_eq!(state.diff_scroll, DIFF_SCROLL_STEP);
        handle_key(&mut state, &traces, KeyCode::Char('k'), 20, 4);
        assert_eq!(state.diff_scroll, 0);
    }

    #[test]
    fn enter_on_traces_selects_entries_pane() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Traces;
        handle_key(&mut state, &traces, KeyCode::Enter, 0, 0);
        assert_eq!(state.focus, Focus::Entries);
        handle_key(&mut state, &traces, KeyCode::Enter, 0, 0);
        assert_eq!(state.focus, Focus::Entries);
    }

    #[test]
    fn pane_at_returns_correct_focus() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let state = state_with_rects(&traces);
        assert_eq!(pane_at(&state, 5, 3), Some(Focus::Entries));
        assert_eq!(pane_at(&state, 5, 15), Some(Focus::Traces));
        assert_eq!(pane_at(&state, 50, 5), Some(Focus::Diff));
        assert_eq!(pane_at(&state, 200, 200), None);
    }

    #[test]
    fn scroll_on_diff_pane_scrolls_diff() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        let mouse = make_mouse(MouseEventKind::ScrollDown, 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.diff_scroll, DIFF_MOUSE_SCROLL_STEP);
        let mouse = make_mouse(MouseEventKind::ScrollUp, 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.diff_scroll, 0);
    }

    #[test]
    fn scroll_at_bounds_is_noop() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        let mouse = make_mouse(MouseEventKind::ScrollUp, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 0);
    }

    #[test]
    fn click_focuses_pane() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.focus = Focus::Entries;

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.focus, Focus::Diff);

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 15);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.focus, Focus::Traces);

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.focus, Focus::Entries);
    }

    #[test]
    fn click_outside_panes_is_noop() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.focus = Focus::Entries;
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 200, 200);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.focus, Focus::Entries);
    }
}
