//! Terminal UI for browsing edit/write traces for the current directory.
//!
//! Layout (lazygit-inspired):
//! - Left sidebar, top: entries (edits/writes) of the selected trace.
//! - Left sidebar, bottom: traces for the current working directory.
//! - Right: diff / detail for the selected entry.
//!
//! Focus toggles between the traces pane and the entries pane with `Tab`.
//!
//! The left column (entries + traces) is a resizable sidebar: `<`/`>`
//! narrow/widen it from any focus, or drag the divider between the panes
//! with the mouse. Its default width scales with the terminal and it
//! never hides, clamping to a minimum on narrow terminals.
//!
//! ## Module layout
//!
//! Split by change axis, mirroring `cli/review/`. This file is the shell:
//! it owns the entry point, the event loop, event routing to panes,
//! layout, the divider drag, and timing. Each pane owns its movement and
//! render:
//!
//! - [`model`] — load traces/entries for the current directory.
//! - [`entries_pane`] / [`traces_pane`] — the two list slices.
//! - [`detail`] — the detail/diff slice (cache + renderers).
//! - [`reload`] — reload from disk, preserving selection.
//! - [`scripted`] — the headless (non-TTY) render path.

use std::io::{self, IsTerminal};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::Style,
    widgets::{ListState, Paragraph},
};

use crate::events::read_event_burst;
use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar_width::Preference;
use crate::terminal::TerminalSession;
use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_block_with_footer, position_footer, rgb_to_color};

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
const POLL_TIMEOUT: Duration = Duration::from_secs(2);
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// Entry point. Loads traces for the current directory and opens the TUI
/// (or renders a scripted view when stdout is not a terminal).
pub fn run() -> Result<(), String> {
    let cwd = current_cwd_or_empty();
    let loaded = load_traces_for_cwd(&cwd)?;
    let theme = Theme::load();

    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        run_tui(loaded, &cwd, &theme)
    } else {
        run_scripted(&loaded, &theme)
    }
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
    /// User's preferred sidebar width plus the policy to resolve it to an
    /// on-screen width each frame. Adjusted by `<`/`>` or by dragging the
    /// divider; clamped on use by [`Preference::effective`].
    sidebar_pref: Preference,
    /// True while the left button is held on the pane divider, so
    /// subsequent `Drag` events resize the sidebar.
    dragging_divider: bool,
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
            sidebar_pref: Preference::seeded(0),
            dragging_divider: false,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    Continue,
    Quit,
}

fn handle_key(
    state: &mut AppState,
    traces: &[LoadedTrace],
    key: KeyCode,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    match key {
        KeyCode::Char('q') => AppCommand::Quit,
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
        // Resize the sidebar regardless of focus, matching review. The
        // stored value is the raw preference; clamping happens in
        // `Preference::effective` at draw time.
        KeyCode::Char('>') => {
            state.sidebar_pref.widen();
            AppCommand::Continue
        }
        KeyCode::Char('<') => {
            state.sidebar_pref.narrow();
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

/// The two adjacent border columns that form the visible divider
/// between the left column (entries/traces) and the diff pane: the left
/// column's right border and the diff's left border. Both `entries_rect`
/// and `traces_rect` share the left column's x/width, so either gives the
/// boundary. `None` when the left column has zero width.
fn divider_columns(state: &AppState) -> Option<(u16, u16)> {
    if state.entries_rect.width == 0 {
        return None;
    }
    let right_border = state.entries_rect.right().saturating_sub(1);
    Some((right_border, right_border.saturating_add(1)))
}

fn is_on_divider(state: &AppState, col: u16) -> bool {
    matches!(divider_columns(state), Some((a, b)) if col == a || col == b)
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
    // Divider drag takes precedence over pane dispatch so a grab on the
    // border neither selects a list row nor changes focus.
    match mouse.kind {
        MouseEventKind::Up(MouseButton::Left) => {
            state.dragging_divider = false;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_divider => {
            state.sidebar_pref.set_from_divider(mouse.column);
            return AppCommand::Continue;
        }
        MouseEventKind::Down(MouseButton::Left) if is_on_divider(state, mouse.column) => {
            state.dragging_divider = true;
            return AppCommand::Continue;
        }
        _ => {}
    }

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

fn app_command_for_event(
    state: &mut AppState,
    traces: &[LoadedTrace],
    event: Event,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(state, traces, key.code, detail_row_count, detail_height)
        }
        Event::Mouse(mouse) => handle_mouse(state, traces, mouse, detail_row_count, detail_height),
        _ => AppCommand::Continue,
    }
}

/// Apply a batch of input events to `state`, stopping early on `Quit`.
///
/// The TUI loop collects every event already buffered before redrawing so a
/// burst of key repeats (e.g. holding `j`) collapses into a single redraw
/// instead of one redraw per repeat.
///
/// Sidebar-resize keys (`<`/`>`) are additionally coalesced to a single
/// step per burst. Holding the key fires OS auto-repeat, which buffers a
/// backlog of presses; applying every one would overshoot and keep growing
/// the sidebar after the key is released. One step per burst makes a hold
/// resize at a steady, frame-paced rate. Mouse drag needs no such guard: it
/// sets an absolute width, so the last event in the burst already wins.
fn apply_events(
    state: &mut AppState,
    traces: &[LoadedTrace],
    events: impl IntoIterator<Item = Event>,
    detail_row_count: usize,
    detail_height: usize,
) -> AppCommand {
    let mut resized = false;
    for event in events {
        if is_resize_key(&event) {
            if resized {
                continue;
            }
            resized = true;
        }
        if app_command_for_event(state, traces, event, detail_row_count, detail_height)
            == AppCommand::Quit
        {
            return AppCommand::Quit;
        }
    }
    AppCommand::Continue
}

/// A key-press of `<` or `>` (the sidebar-resize bindings).
fn is_resize_key(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(key)
            if key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Char('<') | KeyCode::Char('>'))
    )
}

fn run_tui(mut traces: Vec<LoadedTrace>, cwd: &str, theme: &Theme) -> Result<(), String> {
    let _session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("Failed to create screen: {err}"))?;

    // Watch the trace root directory for changes.
    let (notify_tx, notify_rx) = mpsc::channel();
    let trace_root = crate::trace_root_directory()?;
    std::fs::create_dir_all(&trace_root).map_err(|err| {
        format!(
            "Failed to create trace directory {}: {}",
            trace_root.display(),
            err
        )
    })?;
    let mut _watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
        let _ = notify_tx.send(());
    })
    .map_err(|err| format!("Failed to create filesystem watcher: {err}"))?;
    _watcher
        .watch(&trace_root, RecursiveMode::Recursive)
        .map_err(|err| format!("Failed to watch {}: {}", trace_root.display(), err))?;

    let mut state = AppState::new(traces.len());
    let initial_total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    state.sidebar_pref = Preference::seeded(initial_total_width);
    let mut dirty_since: Option<Instant> = None;

    loop {
        let (detail_row_count, detail_height) = terminal
            .draw(|frame| draw(frame, &traces, &mut state, theme))
            .map(|completed| {
                let detail_row_count = state
                    .diff_cache
                    .as_ref()
                    .map(|cache| cache.lines.len())
                    .unwrap_or(0);
                let height = completed.area.height.saturating_sub(3) as usize;
                (detail_row_count, height)
            })
            .map_err(|err| format!("Failed to render screen: {err}"))?;

        // Choose a shorter poll timeout when a debounce is pending so we
        // reload promptly after the debounce window expires.
        let timeout = match dirty_since {
            Some(since) => DEBOUNCE_DELAY.saturating_sub(since.elapsed()),
            None => POLL_TIMEOUT,
        };

        let burst = read_event_burst(timeout)?;
        if apply_events(&mut state, &traces, burst, detail_row_count, detail_height)
            == AppCommand::Quit
        {
            break;
        }

        // Drain all pending filesystem notifications.
        while notify_rx.try_recv().is_ok() {
            dirty_since.get_or_insert_with(Instant::now);
        }

        // Reload once the debounce window has elapsed.
        if dirty_since.is_some_and(|since| since.elapsed() >= DEBOUNCE_DELAY) {
            reload_traces(&mut traces, &mut state, cwd)?;
            dirty_since = None;
        }
    }

    Ok(())
}

fn render_empty_draw_state(
    frame: &mut ratatui::Frame<'_>,
    root: &[ratatui::layout::Rect],
    sidebar: &[ratatui::layout::Rect],
    body: &[ratatui::layout::Rect],
    theme: &Theme,
) {
    let message = Paragraph::new("No traces found for this directory.")
        .style(Style::default().fg(rgb_to_color(theme.muted)))
        .block(pane_block_with_footer(
            "─[1]─Entries─",
            rgb_to_color(theme.border),
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
    frame.render_widget(
        pane_block("─[3]─Diff─", rgb_to_color(theme.border)),
        body[1],
    );
    frame.render_widget(help_bar(theme), root[1]);
}

fn draw(
    frame: &mut ratatui::Frame<'_>,
    traces: &[LoadedTrace],
    state: &mut AppState,
    theme: &Theme,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let sidebar_w = state.sidebar_pref.effective(root[0].width);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_w), Constraint::Min(10)])
        .split(root[0]);

    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[0]);

    if traces.is_empty() {
        render_empty_draw_state(frame, &root, &sidebar, &body, theme);
        return;
    }

    state.entries_rect = sidebar[0];
    state.traces_rect = sidebar[1];
    state.diff_rect = body[1];

    let active_trace = &traces[state.trace_index];

    render_entries_pane(frame, sidebar[0], active_trace, state, theme);
    render_traces_pane(frame, sidebar[1], traces, state, theme);
    render_diff_pane(frame, body[1], active_trace, state, theme);
    frame.render_widget(help_bar(theme), root[1]);
}

fn help_bar(theme: &Theme) -> Paragraph<'static> {
    Paragraph::new(
        "Tab/1/2/3 focus  j/k move  Shift+J/K or PgUp/PgDn scroll diff  < / > resize  q quit",
    )
    .style(Style::default().fg(rgb_to_color(theme.muted)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn tab_cycles_focus() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "Update x"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(state.focus, Focus::Entries);

        let command = handle_key(&mut state, &traces, KeyCode::Tab, 0, 0);
        assert_eq!(command, AppCommand::Continue);
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
        state.focus = Focus::Traces;

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
    fn apply_events_advances_state_once_per_event() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 4, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(state.entry_index(), 0);

        let burst = vec![
            key_press(KeyCode::Char('j')),
            key_press(KeyCode::Char('j')),
            key_press(KeyCode::Char('j')),
        ];
        let command = apply_events(&mut state, &traces, burst, 0, 0);

        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.entry_index(), 3);
    }

    #[test]
    fn apply_events_quit_short_circuits_remaining_burst() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 4, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = AppState::new(traces.len());

        let burst = vec![
            key_press(KeyCode::Char('j')),
            key_press(KeyCode::Char('q')),
            key_press(KeyCode::Char('j')),
        ];
        let command = apply_events(&mut state, &traces, burst, 0, 0);

        assert_eq!(command, AppCommand::Quit);
        // Only the first j was applied; the second j after q must not run.
        assert_eq!(state.entry_index(), 1);
    }

    #[test]
    fn apply_events_empty_burst_is_noop() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.set_entry_index(1);
        let before_focus = state.focus;

        let command = apply_events(&mut state, &traces, std::iter::empty(), 0, 0);

        assert_eq!(command, AppCommand::Continue);
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.focus, before_focus);
    }

    #[test]
    fn app_command_for_event_handles_key_press() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());

        let command =
            app_command_for_event(&mut state, &traces, key_press(KeyCode::Char('q')), 0, 0);

        assert_eq!(command, AppCommand::Quit);
    }

    #[test]
    fn q_quits() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(
            handle_key(&mut state, &traces, KeyCode::Char('q'), 0, 0),
            AppCommand::Quit
        );
    }

    #[test]
    fn esc_does_not_quit() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(
            handle_key(&mut state, &traces, KeyCode::Esc, 0, 0),
            AppCommand::Continue
        );
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

        // Enter on Entries does nothing.
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
        state.diff_scroll = 0;

        let mouse = make_mouse(MouseEventKind::ScrollDown, 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.diff_scroll, DIFF_MOUSE_SCROLL_STEP);

        let mouse = make_mouse(MouseEventKind::ScrollUp, 50, 5);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.diff_scroll, 0);
    }

    #[test]
    fn diff_burst_scroll_applies_every_event() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.diff_scroll = 0;

        let burst = vec![
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 50, 5)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 50, 5)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 50, 5)),
        ];
        apply_events(&mut state, &traces, burst, 20, 10);
        assert_eq!(state.diff_scroll, 3 * DIFF_MOUSE_SCROLL_STEP);
    }

    #[test]
    fn mixed_burst_collapses_each_list_run_independently() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
                entries: vec![edit_entry(), edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);

        let burst = vec![
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 3)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 3)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 15)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 15)),
        ];
        apply_events(&mut state, &traces, burst, 20, 10);
        assert_eq!(state.trace_index, 1);
        // The entries run advanced trace 0's selection by exactly one before
        // the traces run switched the active trace.
        assert_eq!(state.entry_indices[0], 1);
    }

    #[test]
    fn scroll_at_bounds_is_noop() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.entry_index(), 0);

        let mouse = make_mouse(MouseEventKind::ScrollUp, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 0);

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 3);
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

    #[test]
    fn handle_key_grow_and_shrink_sidebar() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.sidebar_pref = Preference::seeded(200);
        let initial = state.sidebar_pref.effective(200);
        handle_key(&mut state, &traces, KeyCode::Char('>'), 0, 0);
        assert!(state.sidebar_pref.effective(200) > initial);
        handle_key(&mut state, &traces, KeyCode::Char('<'), 0, 0);
        assert_eq!(state.sidebar_pref.effective(200), initial);
    }

    #[test]
    fn handle_key_shrink_sidebar_floors_at_min() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.sidebar_pref = Preference::seeded(200);
        for _ in 0..20 {
            handle_key(&mut state, &traces, KeyCode::Char('<'), 0, 0);
        }
        let floored = state.sidebar_pref.effective(200);
        handle_key(&mut state, &traces, KeyCode::Char('<'), 0, 0);
        assert_eq!(state.sidebar_pref.effective(200), floored);
    }

    #[test]
    fn apply_events_coalesces_repeated_resize_keys() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.sidebar_pref = Preference::seeded(200);
        let initial = state.sidebar_pref.effective(200);
        let burst = vec![
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
        ];
        apply_events(&mut state, &traces, burst, 0, 0);
        // One step (4 cols) per burst, not one per repeat.
        assert_eq!(state.sidebar_pref.effective(200), initial + 4);
    }

    #[test]
    fn divider_drag_resizes_and_release_ends() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.sidebar_pref = Preference::seeded(200);
        // entries_rect = (0,0,30,10): divider columns are 29 and 30.
        assert!(is_on_divider(&state, 29));
        assert!(is_on_divider(&state, 30));
        assert!(!is_on_divider(&state, 5));

        // Grab the divider, then drag right to column 50 -> width 51.
        handle_mouse(
            &mut state,
            &traces,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 29, 5),
            20,
            10,
        );
        assert!(state.dragging_divider);
        handle_mouse(
            &mut state,
            &traces,
            make_mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5),
            20,
            10,
        );
        assert_eq!(state.sidebar_pref.effective(200), 51);
        // Drag far left -> floored at the minimum (24).
        handle_mouse(
            &mut state,
            &traces,
            make_mouse(MouseEventKind::Drag(MouseButton::Left), 2, 5),
            20,
            10,
        );
        assert_eq!(state.sidebar_pref.effective(200), 24);
        // Release ends the drag.
        handle_mouse(
            &mut state,
            &traces,
            make_mouse(MouseEventKind::Up(MouseButton::Left), 2, 5),
            20,
            10,
        );
        assert!(!state.dragging_divider);
    }

    #[test]
    fn divider_press_does_not_select_or_change_focus() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
                entries: vec![edit_entry(), edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        let entry = state.entry_index();
        let trace = state.trace_index;
        let focus = state.focus;

        handle_mouse(
            &mut state,
            &traces,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 29, 5),
            20,
            10,
        );
        assert!(state.dragging_divider);
        assert_eq!(state.entry_index(), entry);
        assert_eq!(state.trace_index, trace);
        assert_eq!(state.focus, focus);
    }
}
