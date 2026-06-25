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

use std::collections::HashSet;
use std::io::{self, IsTerminal, Read};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};
use unicode_width::UnicodeWidthChar;

use crate::events::read_event_burst;
use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar_width::{self, Preference};
use crate::terminal::TerminalSession;
use crate::{HistoryEntry, TraceSummary, list_traces_for_current_directory, read_history_entries};
use deltoids::render_tui::{
    self, pane_block, pane_block_with_footer, pane_border_color, pane_inner_height,
    position_footer, render_pane_scrollbar, rgb_to_color,
};
use deltoids::{Hunk, Theme};

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

fn load_traces_for_cwd(cwd: &str) -> Result<Vec<LoadedTrace>, String> {
    let traces = list_traces_for_current_directory()?;
    let mut loaded = Vec::with_capacity(traces.len());
    for trace in traces {
        let entries = read_history_entries(&trace.trace_id)?
            .into_iter()
            .filter(|entry| entry.cwd == cwd)
            .collect::<Vec<_>>();
        loaded.push(LoadedTrace { trace, entries });
    }
    Ok(loaded)
}

fn current_cwd_or_empty() -> String {
    std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
struct LoadedTrace {
    trace: TraceSummary,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Traces,
    Entries,
    Diff,
}

#[derive(Debug, Clone)]
struct DiffCache {
    trace_index: usize,
    entry_index: usize,
    width: usize,
    lines: Vec<Line<'static>>,
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

fn move_entry_down(state: &mut AppState, traces: &[LoadedTrace]) {
    let entry_count = traces
        .get(state.trace_index)
        .map(|trace| trace.entries.len())
        .unwrap_or(0);
    let current = state.entry_index();
    if current + 1 < entry_count {
        state.set_entry_index(current + 1);
        state.diff_scroll = 0;
    }
}

fn move_entry_up(state: &mut AppState) {
    let current = state.entry_index();
    if current > 0 {
        state.set_entry_index(current - 1);
        state.diff_scroll = 0;
    }
}

fn move_trace_down(state: &mut AppState, traces: &[LoadedTrace]) {
    if state.trace_index + 1 < traces.len() {
        state.trace_index += 1;
        state.traces_list_state.select(Some(state.trace_index));
        state.diff_scroll = 0;
    }
}

fn move_trace_up(state: &mut AppState) {
    if state.trace_index > 0 {
        state.trace_index -= 1;
        state.traces_list_state.select(Some(state.trace_index));
        state.diff_scroll = 0;
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

fn max_detail_scroll(detail_row_count: usize, detail_height: usize) -> usize {
    detail_row_count.saturating_sub(detail_height.max(1))
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

/// Reload traces from disk unconditionally. When a new trace appears at
/// index 0 (newest), automatically switches to it. Otherwise preserves the
/// current selection by trace id and entry index when the selected trace
/// still exists; falls back to index 0 otherwise.
fn reload_traces(
    traces: &mut Vec<LoadedTrace>,
    state: &mut AppState,
    cwd: &str,
) -> Result<(), String> {
    // Collect known trace IDs before reload.
    let known_ids: HashSet<_> = traces.iter().map(|t| t.trace.trace_id.as_str()).collect();

    let new_traces = load_traces_for_cwd(cwd)?;

    // Check if the newest trace is new (unknown before reload).
    let newest_is_new = new_traces
        .first()
        .is_some_and(|t| !known_ids.contains(t.trace.trace_id.as_str()));

    // Remember current selection.
    let prev_trace_id = traces
        .get(state.trace_index)
        .map(|t| t.trace.trace_id.clone());
    let prev_entry_index = state.entry_index();

    // Replace traces.
    *traces = new_traces;

    // Rebuild entry_indices for the new trace count.
    state.entry_indices = vec![0; traces.len()];

    if newest_is_new {
        // New trace arrived: switch to it.
        state.trace_index = 0;
        state.traces_list_state.select(Some(0));
        state.set_entry_index(0);
        state.diff_scroll = 0;
        state.diff_cache = None;
        return Ok(());
    }

    // Restore trace selection by id, or fall back to 0.
    state.trace_index = prev_trace_id
        .as_deref()
        .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
        .unwrap_or(0);
    state.traces_list_state.select(Some(state.trace_index));

    // Restore entry index, clamped to the new entry count.
    let entry_count = traces
        .get(state.trace_index)
        .map(|t| t.entries.len())
        .unwrap_or(0);
    let clamped = if entry_count == 0 {
        0
    } else {
        prev_entry_index.min(entry_count - 1)
    };
    state.set_entry_index(clamped);

    // Invalidate caches.
    state.diff_cache = None;

    // Reset scroll only when the selected entry changed (trace disappeared
    // or entry index was clamped). When the same entry is still selected the
    // user may be reviewing the diff, so preserve their scroll position.
    let selection_changed = prev_trace_id.as_deref()
        != traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.as_str())
        || clamped != prev_entry_index;
    if selection_changed {
        state.diff_scroll = 0;
    }

    Ok(())
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

fn diff_cache_matches_selection_and_width(
    cache: &DiffCache,
    trace_index: usize,
    entry_index: usize,
    width: usize,
) -> bool {
    cache.trace_index == trace_index && cache.entry_index == entry_index && cache.width == width
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

fn render_entries_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    theme: &Theme,
) {
    let entry_items = active_trace
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_label_line(entry)))
        .collect::<Vec<_>>();
    let entries_count = active_trace.entries.len();
    let entries_position = if entries_count == 0 {
        0
    } else {
        state.entry_index() + 1
    };
    let entries_list = List::new(entry_items)
        .block(pane_block_with_footer(
            "─[1]─Entries─",
            pane_border_color(state.focus == Focus::Entries, theme),
            Some(position_footer(entries_position, entries_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(rgb_to_color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .scroll_padding(2);
    frame.render_stateful_widget(entries_list, area, &mut state.entries_list_state);
    render_pane_scrollbar(
        frame,
        area,
        entries_count,
        state.entry_index(),
        pane_inner_height(area),
        theme,
    );
}

fn render_traces_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    traces: &[LoadedTrace],
    state: &mut AppState,
    theme: &Theme,
) {
    let trace_items = traces
        .iter()
        .map(|loaded| ListItem::new(trace_label(&loaded.trace)))
        .collect::<Vec<_>>();
    let traces_count = traces.len();
    let traces_position = if traces_count == 0 {
        0
    } else {
        state.trace_index + 1
    };
    let traces_list = List::new(trace_items)
        .block(pane_block_with_footer(
            "─[2]─Traces─",
            pane_border_color(state.focus == Focus::Traces, theme),
            Some(position_footer(traces_position, traces_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(rgb_to_color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .scroll_padding(2);
    frame.render_stateful_widget(traces_list, area, &mut state.traces_list_state);
    render_pane_scrollbar(
        frame,
        area,
        traces_count,
        state.trace_index,
        pane_inner_height(area),
        theme,
    );
}

fn ensure_diff_cache(
    active_trace: &LoadedTrace,
    state: &mut AppState,
    detail_width: usize,
    theme: &Theme,
) {
    let entry_index = state.entry_index();
    let cache_valid = state.diff_cache.as_ref().is_some_and(|cache| {
        diff_cache_matches_selection_and_width(cache, state.trace_index, entry_index, detail_width)
    });
    if !cache_valid {
        let lines = render_detail_for(active_trace, entry_index, detail_width, theme);
        state.diff_cache = Some(DiffCache {
            trace_index: state.trace_index,
            entry_index,
            width: detail_width,
            lines,
        });
    }
}

fn render_diff_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    theme: &Theme,
) {
    let detail_width = area.width.saturating_sub(2) as usize;
    ensure_diff_cache(active_trace, state, detail_width, theme);

    let diff_viewport = area.height.saturating_sub(2) as usize;
    let detail_row_count = state
        .diff_cache
        .as_ref()
        .map(|cache| cache.lines.len())
        .unwrap_or(0);
    let start = state.diff_scroll.min(detail_row_count);
    let end = start
        .saturating_add(diff_viewport.max(1))
        .min(detail_row_count);
    let visible_lines: Vec<Line<'static>> = state
        .diff_cache
        .as_ref()
        .map(|cache| cache.lines[start..end].to_vec())
        .unwrap_or_default();
    let diff = Paragraph::new(visible_lines).block(pane_block(
        "─[3]─Diff─",
        pane_border_color(state.focus == Focus::Diff, theme),
    ));
    frame.render_widget(diff, area);

    render_pane_scrollbar(
        frame,
        area,
        detail_row_count,
        state.diff_scroll,
        diff_viewport,
        theme,
    );
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

fn entry_icon(ok: bool) -> (&'static str, Color) {
    if ok {
        ("\u{2713}", Color::Green)
    } else {
        ("\u{2717}", Color::Red)
    }
}

fn file_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn entry_label_line(entry: &HistoryEntry) -> Line<'static> {
    let (icon, icon_color) = entry_icon(entry.ok);
    let basename = file_basename(&entry.path);
    Line::from(vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(format!(" {basename} ")),
        Span::styled(entry.reason.clone(), Style::default().fg(Color::DarkGray)),
    ])
}

fn entry_label_plain(entry: &HistoryEntry) -> String {
    let (icon, _) = entry_icon(entry.ok);
    format!("{icon} {} {}", file_basename(&entry.path), entry.reason)
}

fn trace_label(summary: &TraceSummary) -> String {
    let short_id = short_trace_id(&summary.trace_id);
    format!(
        "{}  {} entries  {}  {}",
        short_id, summary.entry_count, summary.last_timestamp, summary.last_reason
    )
}

fn short_trace_id(trace_id: &str) -> String {
    if trace_id.len() <= 10 {
        trace_id.to_string()
    } else {
        trace_id[..10].to_string()
    }
}

/// One unit of the entry detail view, ready to be rendered.
///
/// Built once per visible entry; `render_detail_for` walks the items and
/// dispatches each variant to the right renderer. Lifetimes borrow from
/// the originating `HistoryEntry`, so building this list is allocation
/// light.
#[derive(Debug)]
enum DetailItem<'a> {
    /// v1 trace entry: hunks were not recorded, only legacy diff text.
    OldFormatNotice,
    /// Failure entry's error message.
    ErrorLine(&'a str),
    /// Edit-tool reasons to render before the next hunk.
    EditBlock(Vec<&'a str>),
    /// Blank line between hunks.
    HunkSpacer,
    /// One full hunk (header + context + subhunks). Rendered via
    /// [`deltoids::render_tui::render_hunk`].
    Hunk(&'a Hunk),
}

/// Build the structured detail view for a history entry.
///
/// Walks `entry.hunks` directly; emits edit reasons, hunk headers,
/// context, and subhunks in the order the renderer needs them.
fn detail_items(entry: &HistoryEntry) -> Vec<DetailItem<'_>> {
    if !entry.ok {
        return entry
            .error
            .as_deref()
            .map(|err| vec![DetailItem::ErrorLine(err)])
            .unwrap_or_default();
    }

    if entry.hunks.is_empty() {
        // v1 entries have no hunks; show deprecation notice.
        return vec![DetailItem::OldFormatNotice];
    }

    let mut items = Vec::new();
    let hunk_count = entry.hunks.len();
    let mut next_edit_index = 0usize;

    for (hunk_index, hunk) in entry.hunks.iter().enumerate() {
        if hunk_index > 0 {
            items.push(DetailItem::HunkSpacer);
        }

        if !entry.edits.is_empty() {
            let remaining_hunks = hunk_count.saturating_sub(hunk_index);
            let remaining_edits = entry.edits.len().saturating_sub(next_edit_index);
            let edits_for_this_hunk = if remaining_edits == 0 {
                0
            } else if remaining_edits <= remaining_hunks {
                1
            } else {
                remaining_edits - (remaining_hunks - 1)
            };
            if edits_for_this_hunk > 0 {
                let reasons: Vec<&str> = entry.edits
                    [next_edit_index..next_edit_index + edits_for_this_hunk]
                    .iter()
                    .map(|edit| edit.reason.as_str())
                    .collect();
                items.push(DetailItem::EditBlock(reasons));
                next_edit_index += edits_for_this_hunk;
            }
        }

        items.push(DetailItem::Hunk(hunk));
    }

    items
}

fn diff_hunk_count(entry: &HistoryEntry) -> usize {
    entry.hunks.len()
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn collapse_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    if home.is_empty() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(home.as_ref()) {
        if rest.is_empty() {
            return "~".to_string();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn render_detail_for(
    trace: &LoadedTrace,
    entry_index: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };

    let items = detail_items(entry);
    let mut rendered = render_detail_header(entry, width, theme);

    if !rendered.is_empty() && !items.is_empty() {
        rendered.push(Line::from(""));
    }

    for item in items {
        match item {
            DetailItem::OldFormatNotice => {
                rendered.push(Line::from("(old format, cannot display)"));
            }
            DetailItem::ErrorLine(err) => {
                rendered.push(labeled_line("error", err, Color::Red));
            }
            DetailItem::EditBlock(reasons) => {
                rendered.extend(render_edit_block(&reasons, width, theme));
            }
            DetailItem::HunkSpacer => {
                rendered.push(Line::from(""));
            }
            DetailItem::Hunk(hunk) => {
                rendered.extend(render_tui::render_hunk(
                    hunk,
                    entry.highlight.as_deref(),
                    width,
                    theme,
                ));
            }
        }
    }

    rendered
}

fn render_detail_header(entry: &HistoryEntry, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let path = collapse_home(&entry.path);
    let metadata = header_metadata_line(entry);
    render_header_block(&entry.reason, &path, &metadata, width, theme)
}

fn header_metadata_line(entry: &HistoryEntry) -> String {
    let mut parts = vec![
        entry.tool.clone(),
        if entry.ok {
            "ok".to_string()
        } else {
            "error".to_string()
        },
    ];

    if !entry.edits.is_empty() {
        parts.push(count_label(entry.edits.len(), "edit", "edits"));
    }

    parts.push(count_label(diff_hunk_count(entry), "hunk", "hunks"));
    parts.join(" • ")
}

fn labeled_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if ch == '\t' {
                4
            } else {
                ch.width().unwrap_or(0)
            }
        })
        .sum()
}

fn split_word_to_width(word: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0usize;

    for ch in word.chars() {
        let ch_width = if ch == '\t' {
            4
        } else {
            ch.width().unwrap_or(0)
        };
        if chunk_width + ch_width > max_width && !chunk.is_empty() {
            lines.push(chunk);
            chunk = String::new();
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += ch_width;
    }

    if !chunk.is_empty() {
        lines.push(chunk);
    }

    lines
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = display_width(word);

        if word_width > max_width {
            // Word too long for a single line: flush current, then split by character.
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
                current_width = 0;
            }
            let mut chunks = split_word_to_width(word, max_width).into_iter();
            if let Some(last) = chunks.next_back() {
                lines.extend(chunks);
                current_width = display_width(&last);
                current = last;
            }
            continue;
        }

        if current.is_empty() {
            current = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= max_width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn render_header_block(
    reason: &str,
    path: &str,
    metadata: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let reason_style = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);

    if width < 4 {
        return vec![Line::from(Span::styled(
            fit_line(reason, width),
            reason_style,
        ))];
    }

    let path_style = Style::default().fg(rgb_to_color(theme.border));
    let metadata_style = Style::default().fg(rgb_to_color(theme.muted));
    let border = Style::default().fg(rgb_to_color(theme.border));
    let bot = format!("─{}", "─".repeat(width.saturating_sub(1)));

    let mut lines = Vec::new();
    for wrapped in wrap_text(reason, width) {
        lines.push(Line::from(Span::styled(wrapped, reason_style)));
    }
    lines.push(Line::from(Span::styled(fit_line(path, width), path_style)));
    lines.push(Line::from(Span::styled(
        fit_line(metadata, width),
        metadata_style,
    )));
    lines.push(Line::from(Span::styled(bot, border)));
    lines
}

fn render_edit_block(lines: &[&str], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let border = Style::default().fg(rgb_to_color(theme.border_active));
    let content_width = lines
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0)
        .min(width.saturating_sub(2));

    let top = format!("{}╮", "─".repeat(content_width + 1));
    let bot = format!("{}╯", "─".repeat(content_width + 1));
    let mut rendered = vec![Line::from(Span::styled(top, border))];

    for line in lines {
        let fitted = fit_line(line, content_width);
        let padding = content_width.saturating_sub(display_width(&fitted));
        rendered.push(Line::from(vec![
            Span::styled(fitted, border),
            Span::styled(" ".repeat(padding), border),
            Span::styled(" │", border),
        ]));
    }

    rendered.push(Line::from(Span::styled(bot, border)));
    rendered
}

fn fit_line(line: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut result = String::new();
    for ch in line.chars().take(width) {
        result.push(ch);
    }
    result
}

fn run_scripted(traces: &[LoadedTrace], theme: &Theme) -> Result<(), String> {
    let mut state = AppState::new(traces.len());
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    for ch in input.chars() {
        match ch {
            'j' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll += DIFF_SCROLL_STEP;
                } else {
                    move_down(&mut state, traces);
                }
            }
            'k' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP);
                } else {
                    move_up(&mut state, traces);
                }
            }
            '\t' => {
                state.focus = match state.focus {
                    Focus::Entries => Focus::Traces,
                    Focus::Traces => Focus::Diff,
                    Focus::Diff => Focus::Entries,
                };
            }
            'J' => state.diff_scroll += DIFF_SCROLL_STEP,
            'K' => state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP),
            '1' => state.focus = Focus::Entries,
            '2' => state.focus = Focus::Traces,
            '3' => state.focus = Focus::Diff,
            'q' => break,
            _ => {}
        }
    }

    print!("{}", render_scripted(traces, &state, 120, 30, theme));
    Ok(())
}

fn render_scripted(
    traces: &[LoadedTrace],
    state: &AppState,
    width: usize,
    height: usize,
    theme: &Theme,
) -> String {
    if traces.is_empty() {
        return "No traces found for this directory.\n".to_string();
    }

    let left_width = sidebar_width::default_width(width as u16) as usize;
    let right_width = width.saturating_sub(left_width + 3);
    let body_height = height.max(3);
    let sidebar_half = (body_height / 2).max(2);

    let active_trace = &traces[state.trace_index];

    // Top-left: entries list (header + entries, padded/truncated to sidebar_half rows)
    let focus_entries_marker = if state.focus == Focus::Entries {
        "*"
    } else {
        " "
    };
    let entries_count = active_trace.entries.len();
    let entries_position = if entries_count == 0 {
        0
    } else {
        state.entry_index() + 1
    };
    let mut entries_section = vec![format!(
        "{focus_entries_marker} [1] Entries {}",
        position_footer(entries_position, entries_count).trim()
    )];
    for (index, entry) in active_trace.entries.iter().enumerate() {
        let marker = if index == state.entry_index() {
            ">"
        } else {
            " "
        };
        entries_section.push(fit_line(
            &format!("{marker} {}", entry_label_plain(entry)),
            left_width,
        ));
    }

    // Bottom-left: traces list
    let focus_traces_marker = if state.focus == Focus::Traces {
        "*"
    } else {
        " "
    };
    let traces_count = traces.len();
    let traces_position = if traces_count == 0 {
        0
    } else {
        state.trace_index + 1
    };
    let mut traces_section = vec![format!(
        "{focus_traces_marker} [2] Traces {}",
        position_footer(traces_position, traces_count).trim()
    )];
    for (index, loaded) in traces.iter().enumerate() {
        let marker = if index == state.trace_index { ">" } else { " " };
        traces_section.push(fit_line(
            &format!("{marker} {}", trace_label(&loaded.trace)),
            left_width,
        ));
    }

    let entries_rows = pad_or_truncate(&entries_section, sidebar_half);
    let traces_rows = pad_or_truncate(&traces_section, body_height.saturating_sub(sidebar_half));
    let sidebar_rows = [entries_rows, traces_rows].concat();

    // Right: diff for selected entry, spans full body height
    let detail = render_detail_for(active_trace, state.entry_index(), right_width, theme)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let diff_rows = detail
        .iter()
        .skip(state.diff_scroll)
        .take(body_height)
        .map(|line| fit_line(line, right_width))
        .collect::<Vec<_>>();

    let mut output = String::new();
    for row in 0..body_height {
        let left = sidebar_rows.get(row).map(String::as_str).unwrap_or("");
        let right = diff_rows.get(row).map(String::as_str).unwrap_or("");
        output.push_str(&format!("{left:<left_width$} | {right}\n"));
    }

    output
}

fn pad_or_truncate(rows: &[String], target: usize) -> Vec<String> {
    let mut result = rows.iter().take(target).cloned().collect::<Vec<_>>();
    while result.len() < target {
        result.push(String::new());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextEdit;

    fn test_theme() -> Theme {
        Theme::load()
    }

    fn edit_entry() -> HistoryEntry {
        HistoryEntry {
            v: 1,
            tool: "edit".to_string(),
            trace_id: "01JTESTTRACE00000000000000".to_string(),
            timestamp: "2026-04-16T12:00:00Z".to_string(),
            cwd: "/tmp/project".to_string(),
            path: "/tmp/project/app.txt".to_string(),
            reason: "Update x constant".to_string(),
            ok: true,
            edits: vec![TextEdit {
                reason: "Edit change".to_string(),
                old_text: "const x = 1;".to_string(),
                new_text: "const x = 2;".to_string(),
            }],
            content: String::new(),
            diff: Some(
                "--- a/app.txt\n+++ b/app.txt\n@@ -1 +1 @@ fn update() {\n-const x = 1;\n+const x = 2;\n"
                    .to_string(),
            ),
            error: None,
            hunks: Vec::new(),
            language: None,
            highlight: None,
        }
    }

    fn write_entry() -> HistoryEntry {
        HistoryEntry {
            v: 1,
            tool: "write".to_string(),
            trace_id: "01JTESTTRACE00000000000000".to_string(),
            timestamp: "2026-04-16T12:01:00Z".to_string(),
            cwd: "/tmp/project".to_string(),
            path: "/tmp/project/config.json".to_string(),
            reason: "Rewrite config".to_string(),
            ok: true,
            edits: Vec::new(),
            content: "{\n  \"version\": 2\n}\n".to_string(),
            diff: Some(
                "--- a/config.json\n+++ b/config.json\n@@ -1,3 +1,3 @@\n   \"version\": 1\n+  \"version\": 2\n"
                    .to_string(),
            ),
            error: None,
            hunks: Vec::new(),
            language: None,
            highlight: None,
        }
    }

    fn trace_summary(trace_id: &str, entry_count: usize, last_reason: &str) -> TraceSummary {
        TraceSummary {
            trace_id: trace_id.to_string(),
            entry_count,
            last_timestamp: "2026-04-16T12:00:00Z".to_string(),
            last_tool: "edit".to_string(),
            last_path: "/tmp/project/app.txt".to_string(),
            last_reason: last_reason.to_string(),
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
    fn j_moves_traces_when_focused_on_traces() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Traces;

        handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.entry_index(), 0);
    }

    #[test]
    fn j_moves_entries_when_focused_on_entries() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), write_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Entries;

        handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.trace_index, 0);
    }

    fn key_press(code: KeyCode) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ))
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

        let command = app_command_for_event(
            &mut state,
            &traces,
            Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('q'),
                crossterm::event::KeyModifiers::NONE,
            )),
            0,
            0,
        );

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
    fn diff_cache_matches_selection_and_width_checks_all_cache_fields() {
        let cache = DiffCache {
            trace_index: 1,
            entry_index: 2,
            width: 80,
            lines: Vec::new(),
        };

        assert!(diff_cache_matches_selection_and_width(&cache, 1, 2, 80));
        assert!(!diff_cache_matches_selection_and_width(&cache, 0, 2, 80));
        assert!(!diff_cache_matches_selection_and_width(&cache, 1, 0, 80));
        assert!(!diff_cache_matches_selection_and_width(&cache, 1, 2, 79));
    }

    #[test]
    fn scripted_render_shows_traces_and_entries() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 2, "Update x"),
                entries: vec![edit_entry(), write_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "other"),
                entries: vec![edit_entry()],
            },
        ];
        let state = AppState::new(traces.len());

        let theme = test_theme();
        let output = render_scripted(&traces, &state, 140, 30, &theme);

        assert!(output.contains("\u{2713} app.txt"));
        assert!(output.contains("\u{2713} config.json"));
        assert!(output.contains("01JTESTTRA"));
        assert!(output.contains("[1] Entries 1 of 2"));
        assert!(output.contains("[2] Traces 1 of 2"));
        assert!(output.contains("/tmp/project/app.txt"));
        assert!(output.contains("edit • ok • 1 edit • 0 hunks"));
        // v1 entries show deprecation message instead of diff content
        assert!(output.contains("(old format, cannot display)"));
    }

    #[test]
    fn scripted_render_shows_empty_message() {
        let state = AppState::new(0);
        let theme = test_theme();
        let output = render_scripted(&[], &state, 140, 30, &theme);
        assert!(output.contains("No traces"));
    }

    #[test]
    fn detail_items_renders_hunk_with_header_context_and_change() {
        use deltoids::{DiffLine, Hunk, LineKind, ScopeNode};

        let mut entry = edit_entry();
        entry.hunks = vec![Hunk {
            old_start: 5,
            new_start: 5,
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "context line".to_string(),
                },
                DiffLine {
                    kind: LineKind::Removed,
                    content: "old line".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "new line".to_string(),
                },
            ],
            ancestors: vec![ScopeNode {
                kind: "function_item".to_string(),
                name: "my_func".to_string(),
                start_line: 3,
                end_line: 10,
                text: "fn my_func() {".to_string(),
            }],
        }];

        let items = detail_items(&entry);

        // EditBlock (1 edit on 1 hunk) + Hunk (header+body rendered as one
        // unit by deltoids::render_tui::render_hunk) = 2 items.
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], DetailItem::EditBlock(_)));
        match &items[1] {
            DetailItem::Hunk(h) => {
                assert_eq!(h.lines.len(), 3);
                assert_eq!(h.ancestors.len(), 1);
            }
            other => panic!("expected Hunk, got {other:?}"),
        }
    }

    #[test]
    fn detail_items_v1_entry_yields_old_format_notice() {
        let entry = edit_entry(); // v1 entry with empty hunks
        let items = detail_items(&entry);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], DetailItem::OldFormatNotice));
    }

    #[test]
    fn scripted_selection_updates_after_navigation() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "Update x"),
            entries: vec![edit_entry(), write_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Entries;
        move_down(&mut state, &traces);

        let theme = test_theme();
        let output = render_scripted(&traces, &state, 140, 30, &theme);

        assert!(output.contains("> \u{2713} config.json"));
        assert!(output.contains("Rewrite config"));
        assert!(output.contains("write • ok • 0 hunks"));
    }

    #[test]
    fn collapse_home_handles_home_prefix() {
        // SAFETY: single-threaded test module and HOME is only read via
        // collapse_home here.
        unsafe { std::env::set_var("HOME", "/home/alice") };
        assert_eq!(
            collapse_home("/home/alice/project/app.rs"),
            "~/project/app.rs"
        );
        assert_eq!(collapse_home("/home/alice"), "~");
        assert_eq!(
            collapse_home("/home/alice-extra/app.rs"),
            "/home/alice-extra/app.rs"
        );
        assert_eq!(collapse_home("/other/path"), "/other/path");
    }

    #[test]
    fn reload_preserves_selection() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), write_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };

        // Start with two traces, select the second trace at entry 0.
        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1;
        state.set_entry_index(0);
        state.diff_cache = Some(DiffCache {
            trace_index: 1,
            entry_index: 0,
            width: 80,
            lines: vec![],
        });

        // Simulate a reload where trace_b gains an entry.
        let trace_b_updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 2, "b updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        // Swap in the new data (simulates what reload_traces does without disk IO).
        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());
        let prev_entry_index = state.entry_index();

        traces = vec![trace_a.clone(), trace_b_updated];
        state.entry_indices = vec![0; traces.len()];
        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);

        let entry_count = traces
            .get(state.trace_index)
            .map(|t| t.entries.len())
            .unwrap_or(0);
        let clamped = if entry_count == 0 {
            0
        } else {
            prev_entry_index.min(entry_count - 1)
        };
        state.set_entry_index(clamped);
        state.diff_cache = None;

        // Selection stays on the same trace.
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.entry_index(), 0);
        assert!(state.diff_cache.is_none());
    }

    #[test]
    fn reload_handles_removed_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };

        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1; // select trace_b
        state.diff_cache = Some(DiffCache {
            trace_index: 1,
            entry_index: 0,
            width: 80,
            lines: vec![],
        });

        // Simulate trace_b disappearing.
        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());

        traces = vec![trace_a.clone()];
        state.entry_indices = vec![0; traces.len()];
        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);
        state.diff_cache = None;

        // Falls back to index 0 since trace_b is gone.
        assert_eq!(state.trace_index, 0);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000000"
        );
        assert!(state.diff_cache.is_none());
    }

    /// Helper that simulates `reload_traces` selection-restore and scroll
    /// logic without disk IO.
    fn simulate_reload(
        traces: &mut Vec<LoadedTrace>,
        state: &mut AppState,
        new_traces: Vec<LoadedTrace>,
    ) {
        // Collect known trace IDs before reload.
        let known_ids: HashSet<_> = traces.iter().map(|t| t.trace.trace_id.as_str()).collect();

        // Check if the newest trace is new.
        let newest_is_new = new_traces
            .first()
            .is_some_and(|t| !known_ids.contains(t.trace.trace_id.as_str()));

        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());
        let prev_entry_index = state.entry_index();

        *traces = new_traces;
        state.entry_indices = vec![0; traces.len()];

        if newest_is_new {
            // New trace arrived: switch to it.
            state.trace_index = 0;
            state.set_entry_index(0);
            state.diff_scroll = 0;
            state.diff_cache = None;
            return;
        }

        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);

        let entry_count = traces
            .get(state.trace_index)
            .map(|t| t.entries.len())
            .unwrap_or(0);
        let clamped = if entry_count == 0 {
            0
        } else {
            prev_entry_index.min(entry_count - 1)
        };
        state.set_entry_index(clamped);
        state.diff_cache = None;

        let selection_changed = prev_trace_id.as_deref()
            != traces
                .get(state.trace_index)
                .map(|t| t.trace.trace_id.as_str())
            || clamped != prev_entry_index;
        if selection_changed {
            state.diff_scroll = 0;
        }
    }

    #[test]
    fn reload_preserves_scroll_when_selection_unchanged() {
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace.clone()];
        let mut state = AppState::new(traces.len());
        state.diff_scroll = 42;

        // Reload with same trace, same entries.
        let updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![updated]);

        assert_eq!(state.diff_scroll, 42, "scroll should be preserved");
    }

    #[test]
    fn reload_resets_scroll_when_trace_disappears() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone(), trace_b];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1;
        state.set_entry_index(0);
        state.diff_scroll = 15;

        // trace_b disappears.
        simulate_reload(&mut traces, &mut state, vec![trace_a]);

        assert_eq!(state.trace_index, 0);
        assert_eq!(state.diff_scroll, 0, "scroll should reset when trace gone");
    }

    #[test]
    fn reload_switches_to_new_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 0;
        state.set_entry_index(0);
        state.diff_scroll = 10;

        // New trace appears at head (newest).
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![trace_b, trace_a]);

        // Should switch to the new trace at index 0.
        assert_eq!(state.trace_index, 0);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000001"
        );
        assert_eq!(state.entry_index(), 0);
        assert_eq!(state.diff_scroll, 0, "scroll should reset for new trace");
    }

    #[test]
    fn reload_preserves_selection_when_no_new_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1; // select trace_b
        state.set_entry_index(0);
        state.diff_scroll = 15;

        // trace_b gains an entry but no new trace appears.
        let trace_b_updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 2, "b updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![trace_a, trace_b_updated]);

        // Selection should stay on trace_b.
        assert_eq!(state.trace_index, 1);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000001"
        );
        assert_eq!(state.diff_scroll, 15, "scroll should be preserved");
    }

    #[test]
    fn reload_resets_scroll_when_entry_index_clamped() {
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), write_entry(), edit_entry()],
        };
        let mut traces = vec![trace];
        let mut state = AppState::new(traces.len());
        state.set_entry_index(2);
        state.diff_scroll = 20;

        // Entries shrink to 1, so entry_index 2 gets clamped to 0.
        let shrunk = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a shrunk"),
            entries: vec![edit_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![shrunk]);

        assert_eq!(state.entry_index(), 0);
        assert_eq!(
            state.diff_scroll, 0,
            "scroll should reset when entry clamped"
        );
    }

    #[test]
    fn render_detail_header_uses_reason_path_metadata_and_rule() {
        let theme = test_theme();
        let lines = render_detail_header(&edit_entry(), 80, &theme);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].to_string().starts_with("Update x constant"));
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(rgb_to_color(theme.border_active))
        );
        assert!(lines[1].to_string().starts_with("/tmp/project/app.txt"));
        assert_eq!(lines[1].spans[0].style.fg, Some(rgb_to_color(theme.border)));
        // v1 entries have 0 hunks
        assert!(
            lines[2]
                .to_string()
                .starts_with("edit • ok • 1 edit • 0 hunks")
        );
        let bottom = lines[3].to_string();
        assert!(bottom.starts_with('─'));
        assert!(!bottom.contains('╯'), "bottom rule should have no corner");
        assert!(!bottom.contains('│'), "no right border");
    }

    #[test]
    fn render_detail_header_wraps_long_reason() {
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.reason = "This is a long reason that should wrap onto multiple lines".to_string();
        let lines = render_detail_header(&entry, 30, &theme);
        // Reason wraps into multiple lines, then path, metadata, rule.
        assert!(
            lines.len() > 4,
            "long reason should produce more than 4 lines, got {}",
            lines.len()
        );
        // All reason lines are border_active (orange) bold.
        let rule_index = lines
            .iter()
            .position(|l| l.to_string().starts_with('─'))
            .expect("should have a bottom rule");
        for line in &lines[..rule_index - 2] {
            assert_eq!(
                line.spans[0].style.fg,
                Some(rgb_to_color(theme.border_active)),
                "wrapped reason line should be border_active color"
            );
        }
        // No right border on any line.
        for line in &lines {
            assert!(
                !line.to_string().contains('│'),
                "no line should have right border"
            );
        }
    }

    #[test]
    fn render_detail_header_falls_back_cleanly_when_narrow() {
        let theme = test_theme();
        let lines = render_detail_header(&edit_entry(), 3, &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "Upd");
    }

    #[test]
    fn wrap_text_fits_on_one_line() {
        assert_eq!(wrap_text("short", 80), vec!["short"]);
    }

    #[test]
    fn wrap_text_wraps_at_word_boundary() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }

    #[test]
    fn wrap_text_splits_long_word_by_character() {
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_text_empty_string() {
        assert_eq!(wrap_text("", 80), vec![""]);
    }

    #[test]
    fn wrap_text_exact_fit() {
        assert_eq!(wrap_text("abcd", 4), vec!["abcd"]);
    }

    #[test]
    fn wrap_text_single_word_longer_than_width() {
        assert_eq!(wrap_text("abcdef", 4), vec!["abcd", "ef"]);
    }

    #[test]
    fn split_word_to_width_splits_by_display_width() {
        assert_eq!(
            split_word_to_width("abcdefghij", 4),
            vec!["abcd", "efgh", "ij"]
        );
    }

    #[test]
    fn wrap_text_mixed_short_and_long_words() {
        assert_eq!(
            wrap_text("hi abcdefgh there", 6),
            vec!["hi", "abcdef", "gh", "there"]
        );
    }

    #[test]
    fn render_edit_block_uses_border_active_box() {
        let theme = test_theme();
        let lines = render_edit_block(&["Rename renderer"], 80, &theme);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].to_string().starts_with('─'));
        assert!(lines[1].to_string().starts_with("Rename renderer"));
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(rgb_to_color(theme.border_active))
        );
        assert!(lines[2].to_string().ends_with('╯'));
    }

    fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn state_with_rects(traces: &[LoadedTrace]) -> AppState {
        let mut state = AppState::new(traces.len());
        state.entries_rect = Rect::new(0, 0, 30, 10);
        state.traces_rect = Rect::new(0, 10, 30, 10);
        state.diff_rect = Rect::new(30, 0, 90, 20);
        state
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
    fn scroll_down_on_entries_pane_moves_entry_selection() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.focus = Focus::Diff;
        assert_eq!(state.entry_index(), 0);

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.focus, Focus::Diff);
    }

    #[test]
    fn scroll_up_on_entries_pane_moves_entry_selection() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.set_entry_index(2);

        let mouse = make_mouse(MouseEventKind::ScrollUp, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 1);
    }

    #[test]
    fn scroll_down_on_traces_pane_moves_trace_selection() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 15);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.trace_index, 1);
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
    fn entries_burst_scroll_moves_one_item_per_tick() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 4, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.entry_index(), 0);

        let burst = vec![
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 3)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 3)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 3)),
        ];
        apply_events(&mut state, &traces, burst, 20, 10);
        assert_eq!(state.entry_index(), 1);
    }

    #[test]
    fn traces_burst_scroll_moves_one_item_per_tick() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000002", 1, "c"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        let burst = vec![
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 15)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 15)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 15)),
        ];
        apply_events(&mut state, &traces, burst, 20, 10);
        assert_eq!(state.trace_index, 1);
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
    fn click_on_entry_selects_it() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.entry_index(), 0);

        // Click on row 2 inside entries pane (rect starts at y=0, +1 border = row 1 is first item).
        // Row 3 = content_y 2 = item index 2.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 2);
        assert_eq!(state.focus, Focus::Entries);
    }

    #[test]
    fn click_on_trace_selects_it() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        // Click on row 12 inside traces pane (rect starts at y=10, +1 border = row 11 is first item).
        // Row 12 = content_y 1 = item index 1.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 12);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.focus, Focus::Traces);
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
