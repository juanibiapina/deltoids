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
//!
//! Review comments live in the shell-level [`super::comments`] (the pure
//! store/prompt core), [`super::comment_view`] (how they look), and
//! [`super::diff_cursor`] (rows and cursor), all shared with Files mode.
//!
//! With the diff pane focused, `j`/`k` move a cursor between logical diff
//! lines (headers, spacers, and wrapped continuation rows are skipped);
//! `c` comments on the selected line, `d` deletes that comment, and `y`
//! copies every comment on the trace as one agent-ready prompt.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::Style,
    widgets::{ListState, Paragraph},
};

use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use deltoids::render_tui::{
    pane_block, pane_block_with_footer, pane_border_color, position_footer, rgb_to_color,
};
use deltoids::{ChangeLayout, LineKind, Theme};

use super::mode::{AppCommand, DrawBudget, Mode, ReloadViewport, TabStrip};

mod detail;
mod entries_pane;
mod model;
mod reload;
mod scripted;
#[cfg(test)]
mod test_support;
mod traces_pane;

use super::comment_view::render_comment_editor;
use super::comments::{
    CommentAnchor, CommentScope, CommentStore, PromptSection, build_prompt, hunk_lines,
};
use super::diff_cursor::{Cursor, DiffRow, Step, select_row, step_cursor};
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

/// Whether the diff pane is being browsed or a comment is being typed.
#[derive(Debug, Clone)]
enum InputState {
    Normal,
    /// The comment editor is open on `anchor`, holding the text so far
    /// plus the line it annotates. The snapshot is taken when the editor
    /// opens, so the note is never lost if the entry is reloaded while
    /// the reviewer types.
    Commenting {
        anchor: CommentAnchor,
        line: (String, LineKind),
        buffer: String,
    },
}

#[derive(Debug, Clone)]
struct AppState {
    focus: Focus,
    trace_index: usize,
    entry_indices: Vec<usize>,
    /// The diff cursor's row and the pane's scroll offset.
    cursor: Cursor,
    /// Session-only review comments, keyed across all loaded traces.
    comments: CommentStore,
    input: InputState,
    /// Transient Diff-pane footer message (copy feedback).
    status: Option<String>,
    diff_cache: DiffCache,
    /// The rows on screen: the retained render of the selected entry with
    /// this session's comments drawn over it. Rebuilt on each draw, and
    /// what the cursor and scroll math work against.
    window: Vec<DiffRow>,
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
            cursor: Cursor::default(),
            comments: CommentStore::default(),
            input: InputState::Normal,
            status: None,
            diff_cache: DiffCache::default(),
            window: Vec::new(),
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

    /// Send the diff pane back to the top: a different entry's rows have
    /// nothing to do with the previous scroll offset or cursor row.
    fn reset_diff_view(&mut self) {
        self.cursor = Cursor::default();
    }

    /// The `(trace, entry)` cache key for the current selection.
    fn diff_key(&self) -> (usize, usize) {
        (self.trace_index, self.entry_index())
    }

    /// The comment anchor under the diff cursor, when the cursor sits on a
    /// diff line of the last-drawn window.
    fn cursor_anchor(&self) -> Option<CommentAnchor> {
        self.window.get(self.cursor.row)?.anchor.clone()
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

    /// Row count of the last-drawn diff window (0 before the first draw).
    fn detail_row_count(&self) -> usize {
        self.state.window.len()
    }
}

fn handle_key(
    state: &mut AppState,
    traces: &[LoadedTrace],
    key: KeyCode,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    if matches!(state.input, InputState::Commenting { .. }) {
        return handle_comment_key(state, key);
    }

    // Any other key clears the transient copy status.
    state.status = None;

    match key {
        KeyCode::Char('c') => {
            open_comment(state, traces);
            AppCommand::Continue
        }
        KeyCode::Char('d') => {
            delete_comment(state);
            AppCommand::Continue
        }
        KeyCode::Char('y') => copy_comments(state, traces),
        KeyCode::Char('D') => clear_comments(state),
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
            state.cursor.scroll = (state.cursor.scroll + DIFF_SCROLL_STEP).min(max_scroll);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state.cursor.scroll = state.cursor.scroll.saturating_sub(DIFF_SCROLL_STEP);
            AppCommand::Continue
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if state.focus == Focus::Diff {
                move_diff_cursor(state, Step::Down, detail_height);
            } else {
                move_down(state, traces);
            }
            AppCommand::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.focus == Focus::Diff {
                move_diff_cursor(state, Step::Up, detail_height);
            } else {
                move_up(state, traces);
            }
            AppCommand::Continue
        }
        KeyCode::PageDown => {
            let max_scroll = max_detail_scroll(detail_row_count, detail_height);
            state.cursor.scroll = (state.cursor.scroll + detail_height.max(1)).min(max_scroll);
            AppCommand::Continue
        }
        KeyCode::PageUp => {
            state.cursor.scroll = state.cursor.scroll.saturating_sub(detail_height.max(1));
            AppCommand::Continue
        }
        _ => AppCommand::Continue,
    }
}

/// Handle a key while the comment editor is open. The shell routes every
/// key here (see [`Mode::captures_text_input`]), so characters that are
/// normally global bindings are typed as text.
fn handle_comment_key(state: &mut AppState, key: KeyCode) -> AppCommand {
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

/// Store `note` against `anchor`, snapshotting the line it annotates, and
/// rebuild that entry's rows on the next draw. An empty note deletes.
#[cfg(test)]
fn save_comment(state: &mut AppState, traces: &[LoadedTrace], anchor: CommentAnchor, note: String) {
    let Some((code, kind)) = anchored_line(traces, &anchor) else {
        return;
    };
    state.comments.set(anchor, note, code, kind);
}

/// The text and kind of the diff line `anchor` points at, or `None` when
/// the entry or line is gone.
fn anchored_line(traces: &[LoadedTrace], anchor: &CommentAnchor) -> Option<(String, LineKind)> {
    let CommentScope::TraceEntry {
        trace_id,
        entry_index,
    } = &anchor.scope
    else {
        return None;
    };
    let trace = traces.iter().find(|t| &t.trace.trace_id == trace_id)?;
    let entry = trace.entries.get(*entry_index)?;
    entry.hunks.iter().find_map(|hunk| {
        hunk_lines(hunk)
            .find(|line| line.side == anchor.side && line.number == anchor.line)
            .map(|line| (line.content.to_string(), line.kind.clone()))
    })
}

/// Open the comment editor for the diff line under the cursor, seeded
/// with any note already saved there. A no-op unless the diff pane is
/// focused with the cursor on a diff line.
fn open_comment(state: &mut AppState, traces: &[LoadedTrace]) {
    if state.focus != Focus::Diff {
        return;
    }
    let Some(anchor) = state.cursor_anchor() else {
        return;
    };
    let Some(line) = anchored_line(traces, &anchor) else {
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
fn delete_comment(state: &mut AppState) {
    if state.focus != Focus::Diff {
        return;
    }
    let Some(anchor) = state.cursor_anchor() else {
        return;
    };
    state.comments.remove(&anchor);
}

/// Build the review prompt for the selected trace and ask the shell to
/// copy it, recording what happened in the Diff pane footer.
fn copy_comments(state: &mut AppState, traces: &[LoadedTrace]) -> AppCommand {
    let prompt = traces
        .get(state.trace_index)
        .and_then(|trace| trace_prompt(trace, &state.comments));
    match prompt {
        Some((prompt, count)) => {
            let noun = if count == 1 { "comment" } else { "comments" };
            state.status = Some(format!("Copied {count} {noun}"));
            AppCommand::CopyToClipboard(prompt)
        }
        None => {
            state.status = Some("No comments to copy".to_string());
            AppCommand::Continue
        }
    }
}

/// Drop every session comment, recording the count in the footer. Leaves
/// the diff cache untouched: comments are an overlay, so the notes vanish
/// on the next render with no rebuild.
fn clear_comments(state: &mut AppState) -> AppCommand {
    let count = state.comments.clear();
    state.status = Some(if count == 0 {
        "No comments to clear".to_string()
    } else {
        let noun = if count == 1 { "comment" } else { "comments" };
        format!("Cleared {count} {noun}")
    });
    AppCommand::Continue
}

/// Every comment on `trace`, as one prompt ordered by entry then diff
/// position. Paths are shown relative to the entries' working directory.
fn trace_prompt(trace: &LoadedTrace, comments: &CommentStore) -> Option<(String, usize)> {
    let sections: Vec<PromptSection<'_>> = trace
        .entries
        .iter()
        .enumerate()
        .map(|(entry_index, entry)| PromptSection {
            scope: detail::scope(trace, entry_index),
            path: entry.path.clone(),
            hunks: &entry.hunks,
        })
        .collect();
    let cwd = trace
        .entries
        .first()
        .map(|entry| entry.cwd.as_str())
        .unwrap_or_default();
    build_prompt(cwd, comments, &sections)
}

/// Move the diff cursor one diff line, keeping it in view.
fn move_diff_cursor(state: &mut AppState, step: Step, detail_height: usize) {
    let AppState { window, cursor, .. } = state;
    step_cursor(window, step, detail_height, cursor);
}

/// The `path:line` label shown in the comment editor.
fn comment_editor_label(trace: &LoadedTrace, anchor: &CommentAnchor) -> String {
    let path = trace
        .entries
        .iter()
        .find(|entry| entry.path == anchor.path)
        .map(|entry| detail::display_path(&entry.path, &entry.cwd))
        .unwrap_or_else(|| anchor.path.clone());
    format!("{path}:{}", anchor.line)
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
    // The comment editor owns input while it is open.
    if matches!(state.input, InputState::Commenting { .. }) {
        return AppCommand::Continue;
    }
    // Ctrl + wheel redirects the scroll to the entries (edits) list
    // regardless of hover position, so the diff can be scrolled by
    // hovering it while Ctrl steps through entries. (Shift+wheel is
    // swallowed by common terminals/tmux as a mouse-mode bypass, so
    // Ctrl is used instead.)
    let is_scroll = matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    );
    let modified = mouse.modifiers.contains(KeyModifiers::CONTROL);
    let target = if is_scroll && modified {
        Focus::Entries
    } else {
        match pane_at(state, mouse.column, mouse.row) {
            Some(pane) => pane,
            None => return AppCommand::Continue,
        }
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
            state.cursor.scroll =
                (state.cursor.scroll + steps * DIFF_MOUSE_SCROLL_STEP).min(max_scroll);
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
            state.cursor.scroll = state
                .cursor
                .scroll
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
                state.reset_diff_view();
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
                state.reset_diff_view();
            }
        }
        Focus::Diff => {
            // Clicking a diff line moves the cursor there; clicks on
            // headers, spacers, wrapped continuations, and comment rows
            // only focus the pane.
            let content_y = row.saturating_sub(state.diff_rect.y).saturating_sub(1) as usize;
            let clicked = state.cursor.scroll + content_y;
            let AppState { window, cursor, .. } = state;
            select_row(window, clicked, cursor);
        }
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
        layout: ChangeLayout,
        theme: &Theme,
        budget: DrawBudget,
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
        render_diff_pane(
            frame,
            right,
            active_trace,
            &mut self.state,
            layout,
            theme,
            budget,
        );

        if let InputState::Commenting { anchor, buffer, .. } = &self.state.input {
            let label = comment_editor_label(active_trace, anchor);
            render_comment_editor(frame, right, &label, buffer, theme);
        }
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

    fn captures_text_input(&self) -> bool {
        matches!(self.state.input, InputState::Commenting { .. })
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        if let Err(err) = result {
            self.state.status = Some(format!("Copy failed: {err}"));
        }
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

    fn selected_path(&self) -> Option<PathBuf> {
        // The selected entry's path, made absolute by joining its `cwd`
        // when the recorded path is relative. `None` when there are no
        // traces/entries.
        let trace = self.traces.get(self.state.trace_index)?;
        let entry = trace.entries.get(self.state.entry_index())?;
        let path = PathBuf::from(&entry.path);
        if path.is_absolute() {
            Some(path)
        } else {
            Some(PathBuf::from(&entry.cwd).join(path))
        }
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
    use crate::cli::browse::comments::LineSide;
    use crate::cli::browse::traces::test_support::*;
    use detail::CacheEpoch;

    fn grouped_epoch(width: usize) -> CacheEpoch {
        CacheEpoch {
            width,
            layout: ChangeLayout::Grouped,
        }
    }

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
        assert_eq!(state.cursor.scroll, DIFF_SCROLL_STEP);
        state.focus = Focus::Traces;
        handle_key(&mut state, &traces, KeyCode::Char('K'), 20, 4);
        assert_eq!(state.cursor.scroll, 0);
    }

    /// A Traces state focused on the diff pane with one hunk entry
    /// rendered, the cursor snapped onto its first diff line.
    fn diff_state(traces: &[LoadedTrace]) -> AppState {
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Diff;
        rebuild_rows(&mut state, traces);
        state
    }

    /// Save `text` at `anchor` through the same path the editor uses, so
    /// the snapshot and cache invalidation match real usage.
    fn note(state: &mut AppState, traces: &[LoadedTrace], anchor: &CommentAnchor, text: &str) {
        save_comment(state, traces, anchor.clone(), text.to_string());
        // Saving invalidates the entry's rows; a draw rebuilds them.
        rebuild_rows(state, traces);
    }

    /// Re-render the selected entry, as a draw would.
    fn rebuild_rows(state: &mut AppState, traces: &[LoadedTrace]) {
        let key = state.diff_key();
        detail::ensure_diff_cache(
            &traces[state.trace_index],
            state,
            grouped_epoch(80),
            key,
            &test_theme(),
        );
    }

    fn rows_of(state: &AppState) -> Vec<DiffRow> {
        state.window.clone()
    }

    fn row_text(row: &DiffRow) -> String {
        row.line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The `(side, line number)` of the diff line under the cursor.
    fn cursor_line(state: &AppState) -> Option<(LineSide, usize)> {
        state
            .cursor_anchor()
            .map(|anchor| (anchor.side, anchor.line))
    }

    #[test]
    fn j_and_k_move_the_diff_cursor_between_diff_lines() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);

        // The cursor starts on the hunk's first diff line (context, new
        // line 10), past the header, path rule, spacer, and breadcrumb box.
        assert_eq!(cursor_line(&state), Some((LineSide::New, 10)));

        // Then the removed line (old 11) and the added line (new 11).
        handle_key(&mut state, &traces, KeyCode::Char('j'), 40, 10);
        assert_eq!(cursor_line(&state), Some((LineSide::Old, 11)));
        handle_key(&mut state, &traces, KeyCode::Char('j'), 40, 10);
        assert_eq!(cursor_line(&state), Some((LineSide::New, 11)));
        // Past the last diff line the cursor stays put.
        handle_key(&mut state, &traces, KeyCode::Char('j'), 40, 10);
        assert_eq!(cursor_line(&state), Some((LineSide::New, 11)));

        handle_key(&mut state, &traces, KeyCode::Char('k'), 40, 10);
        assert_eq!(cursor_line(&state), Some((LineSide::Old, 11)));
    }

    #[test]
    fn diff_cursor_skips_headers_spacers_and_wrapped_rows() {
        let traces = vec![trace_with_hunks()];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Diff;
        // A narrow pane wraps each diff line onto several rows.
        let key = state.diff_key();
        detail::ensure_diff_cache(
            &traces[0],
            &mut state,
            grouped_epoch(12),
            key,
            &test_theme(),
        );

        let rows = rows_of(&state);
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_selectable())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            selectable.len(),
            3,
            "one stop per diff line, not per rendered row"
        );
        assert!(
            rows.len() > selectable.len() + 3,
            "the narrow pane should have produced wrapped rows"
        );

        // Walking with `j` visits exactly those rows.
        let mut visited = vec![state.cursor.row];
        for _ in 0..2 {
            handle_key(&mut state, &traces, KeyCode::Char('j'), rows.len(), 10);
            visited.push(state.cursor.row);
        }
        assert_eq!(visited, selectable);
    }

    #[test]
    fn moving_the_cursor_keeps_it_inside_the_viewport() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let rows = rows_of(&state).len();
        // A two-row viewport forces the diff to scroll as the cursor moves.
        for _ in 0..2 {
            handle_key(&mut state, &traces, KeyCode::Char('j'), rows, 2);
        }
        assert!(state.cursor.row >= state.cursor.scroll);
        assert!(state.cursor.row < state.cursor.scroll + 2);
    }

    #[test]
    fn c_opens_the_editor_and_enter_saves_the_comment() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let anchor = state.cursor_anchor().unwrap();

        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        assert!(matches!(state.input, InputState::Commenting { .. }));

        for ch in "needs review".chars() {
            handle_key(&mut state, &traces, KeyCode::Char(ch), 40, 10);
        }
        handle_key(&mut state, &traces, KeyCode::Enter, 40, 10);

        assert!(matches!(state.input, InputState::Normal));
        assert_eq!(state.comments.note(&anchor), Some("needs review"));
        // The line it annotates is snapshotted with it.
        assert_eq!(
            state.comments.get(&anchor).map(|c| c.code.as_str()),
            Some("fn main() {")
        );
    }

    #[test]
    fn c_does_nothing_off_a_diff_line() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        state.focus = Focus::Entries;
        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        assert!(matches!(state.input, InputState::Normal));

        // Focused on the diff but parked on a header row.
        state.focus = Focus::Diff;
        state.cursor.row = 0;
        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        assert!(matches!(state.input, InputState::Normal));
    }

    #[test]
    fn c_loads_the_existing_comment_for_editing() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "first pass");

        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        match &state.input {
            InputState::Commenting { buffer, .. } => assert_eq!(buffer, "first pass"),
            other => panic!("expected the editor to open, got {other:?}"),
        }

        // Emptying the buffer and saving removes the comment.
        for _ in 0.."first pass".len() {
            handle_key(&mut state, &traces, KeyCode::Backspace, 40, 10);
        }
        handle_key(&mut state, &traces, KeyCode::Enter, 40, 10);
        assert_eq!(state.comments.note(&anchor), None);
    }

    #[test]
    fn esc_cancels_without_changing_the_saved_comment() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "keep me");

        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        handle_key(&mut state, &traces, KeyCode::Char('x'), 40, 10);
        handle_key(&mut state, &traces, KeyCode::Esc, 40, 10);

        assert!(matches!(state.input, InputState::Normal));
        assert_eq!(state.comments.note(&anchor), Some("keep me"));
    }

    #[test]
    fn d_deletes_the_comment_under_the_cursor() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "note");

        handle_key(&mut state, &traces, KeyCode::Char('d'), 40, 10);
        assert_eq!(state.comments.note(&anchor), None);
    }

    #[test]
    fn saved_comments_render_under_their_diff_line() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);

        handle_key(&mut state, &traces, KeyCode::Char('c'), 40, 10);
        for ch in "explain this".chars() {
            handle_key(&mut state, &traces, KeyCode::Char(ch), 40, 10);
        }
        handle_key(&mut state, &traces, KeyCode::Enter, 40, 10);
        rebuild_rows(&mut state, &traces);

        let rows = rows_of(&state);
        let line_row = rows
            .iter()
            .position(|row| {
                row.anchor
                    .as_ref()
                    .is_some_and(|anchor| anchor.line == 10 && anchor.side == LineSide::New)
            })
            .expect("the commented line is still rendered");
        assert!(
            row_text(&rows[line_row + 1]).contains("explain this"),
            "the comment renders directly under its line"
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
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let key = state.diff_key();
        assert!(
            state.diff_cache.contains(grouped_epoch(80), key),
            "warm after the draw"
        );

        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "explain this");

        assert!(
            state.diff_cache.contains(grouped_epoch(80), key),
            "the render survives a comment"
        );
        assert!(
            rows_of(&state)
                .iter()
                .any(|row| row_text(row).contains("explain this")),
            "and the comment is on screen"
        );
    }

    #[test]
    fn modal_input_captures_characters_that_are_normally_bindings() {
        let traces = vec![trace_with_hunks()];
        let mode_state = diff_state(&traces);
        let mut mode = TracesMode {
            state: mode_state,
            traces: traces.clone(),
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        let anchor = mode.state.cursor_anchor().unwrap();

        assert!(!Mode::captures_text_input(&mode));
        Mode::handle_key(&mut mode, KeyCode::Char('c'), 10, 10);
        assert!(
            Mode::captures_text_input(&mode),
            "the shell must hand every key to the open editor"
        );

        for ch in "q?[]<>d".chars() {
            Mode::handle_key(&mut mode, KeyCode::Char(ch), 10, 10);
        }
        Mode::handle_key(&mut mode, KeyCode::Enter, 10, 10);

        assert!(!Mode::captures_text_input(&mode));
        assert_eq!(mode.state.comments.note(&anchor), Some("q?[]<>d"));
    }

    #[test]
    fn y_reports_what_was_copied() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);

        // Nothing to copy yet.
        let command = handle_key(&mut state, &traces, KeyCode::Char('y'), 40, 10);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to copy"));

        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "look here");
        let command = handle_key(&mut state, &traces, KeyCode::Char('y'), 40, 10);
        match command {
            AppCommand::CopyToClipboard(prompt) => assert!(prompt.contains("note: look here")),
            other => panic!("expected a clipboard copy, got {other:?}"),
        }
        assert_eq!(state.status.as_deref(), Some("Copied 1 comment"));
    }

    #[test]
    fn shift_d_clears_every_comment_and_reports_it() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);

        // Nothing to clear yet.
        let command = handle_key(&mut state, &traces, KeyCode::Char('D'), 40, 10);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to clear"));

        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "look here");
        assert!(!state.comments.is_empty());

        let command = handle_key(&mut state, &traces, KeyCode::Char('D'), 40, 10);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("Cleared 1 comment"));
        assert!(state.comments.is_empty());

        // A following copy has nothing to emit.
        let command = handle_key(&mut state, &traces, KeyCode::Char('y'), 40, 10);
        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.status.as_deref(), Some("No comments to copy"));
    }

    #[test]
    fn a_failed_copy_is_reported_instead_of_success() {
        let traces = vec![trace_with_hunks()];
        let mut mode = TracesMode {
            state: diff_state(&traces),
            traces,
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        mode.state.status = Some("Copied 1 comment".to_string());

        Mode::report_copy(&mut mode, Err("no clipboard".to_string()));
        assert_eq!(
            mode.state.status.as_deref(),
            Some("Copy failed: no clipboard")
        );

        Mode::report_copy(&mut mode, Ok(()));
        assert_eq!(
            mode.state.status.as_deref(),
            Some("Copy failed: no clipboard"),
            "a successful copy leaves the mode's own message in place"
        );
    }

    #[test]
    fn comments_survive_a_reload_with_a_valid_cursor() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        let anchor = state.cursor_anchor().unwrap();
        note(&mut state, &traces, &anchor, "still here");
        state.cursor.row = 999; // as if the previous render had more rows

        // Reload drops every retained render; the next draw rebuilds and
        // re-snaps the cursor.
        state.diff_cache.clear();
        rebuild_rows(&mut state, &traces);

        assert_eq!(state.comments.note(&anchor), Some("still here"));
        assert!(
            state.cursor_anchor().is_some(),
            "the cursor lands back on a diff line"
        );
    }

    #[test]
    fn clicking_a_diff_line_moves_the_cursor_there() {
        let traces = vec![trace_with_hunks()];
        let mut state = diff_state(&traces);
        state.entries_rect = Rect::new(0, 0, 30, 10);
        state.traces_rect = Rect::new(0, 10, 30, 10);
        state.diff_rect = Rect::new(30, 0, 90, 20);

        let rows = rows_of(&state);
        let last_line_row = rows.iter().rposition(DiffRow::is_selectable).unwrap();
        // Row 0 of the pane body is one below the pane's top border.
        let screen_row = (last_line_row + 1) as u16;
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 50, screen_row);
        handle_mouse(&mut state, &traces, mouse, rows.len(), 20);

        assert_eq!(state.cursor.row, last_line_row);
        // A click on a header row only focuses the pane.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 1);
        handle_mouse(&mut state, &traces, mouse, rows.len(), 20);
        assert_eq!(state.cursor.row, last_line_row);
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
        assert_eq!(state.cursor.scroll, DIFF_MOUSE_SCROLL_STEP);
        let mouse = make_mouse(MouseEventKind::ScrollUp, 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.cursor.scroll, 0);
    }

    #[test]
    fn ctrl_scroll_on_diff_moves_entries() {
        // Hovering the diff with Ctrl held redirects the wheel to the
        // entries list instead of scrolling the diff.
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), write_entry()],
        }];
        let mut state = state_with_rects(&traces);
        let initial = state.entry_index();

        // Cursor over the diff (col 50), Ctrl held.
        let mouse = make_mouse_mods(
            MouseEventKind::ScrollDown,
            50,
            5,
            crossterm::event::KeyModifiers::CONTROL,
        );
        handle_mouse(&mut state, &traces, mouse, 20, 10);

        assert!(
            state.entry_index() > initial,
            "ctrl+scroll should move the entries selection"
        );
        assert_eq!(
            state.cursor.scroll, 0,
            "ctrl+scroll should not scroll the diff"
        );
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

    /// Draw one frame of Traces mode and return the flattened screen text.
    fn drawn_screen(mode: &mut TracesMode) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = test_theme();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
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
                TabStrip { active: 1 },
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
    fn the_open_editor_shows_the_target_line_and_typed_text() {
        let traces = vec![trace_with_hunks()];
        let mut mode = TracesMode {
            state: diff_state(&traces),
            traces,
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        // Land on the removed line (old-file line 11) and start typing.
        Mode::handle_key(&mut mode, KeyCode::Char('j'), 10, 18);
        Mode::handle_key(&mut mode, KeyCode::Char('c'), 10, 18);
        for ch in "look here".chars() {
            Mode::handle_key(&mut mode, KeyCode::Char(ch), 10, 18);
        }

        let screen = drawn_screen(&mut mode);
        assert!(screen.contains("app.txt:11"), "screen was:\n{screen}");
        assert!(screen.contains("look here"));
        assert!(screen.contains("Enter save"));
    }

    #[test]
    fn the_diff_pane_footer_shows_the_copy_status() {
        let traces = vec![trace_with_hunks()];
        let mut mode = TracesMode {
            state: diff_state(&traces),
            traces,
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        Mode::handle_key(&mut mode, KeyCode::Char('y'), 10, 18);
        assert!(drawn_screen(&mut mode).contains("No comments to copy"));
    }

    #[test]
    fn selected_path_returns_absolute_entry_path() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mode = TracesMode {
            state: AppState::new(traces.len()),
            traces,
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        // edit_entry's path is already absolute.
        assert_eq!(
            Mode::selected_path(&mode),
            Some(PathBuf::from("/tmp/project/app.txt"))
        );
    }

    #[test]
    fn selected_path_joins_cwd_for_relative_entry() {
        let mut entry = edit_entry();
        entry.cwd = "/tmp/project".to_string();
        entry.path = "src/app.txt".to_string();
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![entry],
        }];
        let mode = TracesMode {
            state: AppState::new(traces.len()),
            traces,
            cwd: "/tmp/project".to_string(),
            _watcher: None,
        };
        assert_eq!(
            Mode::selected_path(&mode),
            Some(PathBuf::from("/tmp/project/src/app.txt"))
        );
    }

    #[test]
    fn selected_path_none_when_empty() {
        assert_eq!(Mode::selected_path(&TracesMode::empty()), None);
    }
}
