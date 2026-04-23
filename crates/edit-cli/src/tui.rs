//! Terminal UI for browsing edit/write traces for the current directory.
//!
//! Layout (lazygit-inspired):
//! - Left sidebar, top: entries (edits/writes) of the selected trace.
//! - Left sidebar, bottom: traces for the current working directory.
//! - Right: diff / detail for the selected entry.
//!
//! Focus toggles between the traces pane and the entries pane with `Tab`.

use std::collections::HashSet;
use std::io::{self, IsTerminal, Read};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    symbols::scrollbar as scrollbar_symbols,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
};
use unicode_width::UnicodeWidthChar;

use crate::highlight::{highlighted_spans, highlighted_spans_with_emphasis};
use crate::theme::{ResolvedTheme, to_color};
use crate::{HistoryEntry, TraceSummary, list_traces_for_current_directory, read_history_entries};
use deltoids::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};

const DIFF_SCROLL_STEP: usize = 3;
const POLL_TIMEOUT: Duration = Duration::from_secs(2);
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// Entry point. Loads traces for the current directory and opens the TUI
/// (or renders a scripted view when stdout is not a terminal).
pub fn run() -> Result<(), String> {
    let cwd = current_cwd_or_empty();
    let loaded = load_traces_for_cwd(&cwd)?;
    let theme = ResolvedTheme::resolve();

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
        _ => AppCommand::Continue,
    }
}

fn move_down(state: &mut AppState, traces: &[LoadedTrace]) {
    match state.focus {
        Focus::Traces => {
            if state.trace_index + 1 < traces.len() {
                state.trace_index += 1;
                state.traces_list_state.select(Some(state.trace_index));
                state.diff_scroll = 0;
            }
        }
        Focus::Entries => {
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
        Focus::Diff => {}
    }
}

fn move_up(state: &mut AppState, _traces: &[LoadedTrace]) {
    match state.focus {
        Focus::Traces => {
            if state.trace_index > 0 {
                state.trace_index -= 1;
                state.traces_list_state.select(Some(state.trace_index));
                state.diff_scroll = 0;
            }
        }
        Focus::Entries => {
            let current = state.entry_index();
            if current > 0 {
                state.set_entry_index(current - 1);
                state.diff_scroll = 0;
            }
        }
        Focus::Diff => {}
    }
}

fn max_detail_scroll(detail_row_count: usize, detail_height: usize) -> usize {
    detail_row_count.saturating_sub(detail_height.max(1))
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
        _ => AppCommand::Continue,
    }
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

fn run_tui(mut traces: Vec<LoadedTrace>, cwd: &str, theme: &ResolvedTheme) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("Failed to create screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

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

        let has_event =
            event::poll(timeout).map_err(|err| format!("Failed to poll input event: {err}"))?;

        if has_event {
            let event =
                event::read().map_err(|err| format!("Failed to read input event: {err}"))?;
            if app_command_for_event(&mut state, &traces, event, detail_row_count, detail_height)
                == AppCommand::Quit
            {
                break;
            }
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

struct TerminalSession;

impl TerminalSession {
    fn enter<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("Failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )
        .map_err(|err| format!("Failed to enter screen: {err}"))?;
        terminal
            .clear()
            .map_err(|err| format!("Failed to clear screen: {err}"))?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
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
    theme: &ResolvedTheme,
) {
    let message = Paragraph::new("No traces found for this directory.")
        .style(Style::default().fg(to_color(theme.ui.muted)))
        .block(pane_block_with_footer(
            " [1] Entries ",
            to_color(theme.ui.border),
            Some(position_footer(0, 0)),
        ));
    frame.render_widget(message, sidebar[0]);
    frame.render_widget(
        pane_block_with_footer(
            " [2] Traces ",
            to_color(theme.ui.border),
            Some(position_footer(0, 0)),
        ),
        sidebar[1],
    );
    frame.render_widget(pane_block(" [3] Diff ", to_color(theme.ui.border)), body[1]);
    frame.render_widget(help_bar(theme), root[1]);
}

fn render_entries_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    theme: &ResolvedTheme,
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
            " [1] Entries ",
            pane_border_color(state.focus == Focus::Entries, theme),
            Some(position_footer(entries_position, entries_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(to_color(theme.ui.selection_bg))
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
    theme: &ResolvedTheme,
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
            " [2] Traces ",
            pane_border_color(state.focus == Focus::Traces, theme),
            Some(position_footer(traces_position, traces_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(to_color(theme.ui.selection_bg))
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
    theme: &ResolvedTheme,
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
    theme: &ResolvedTheme,
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
        " [3] Diff ",
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
    theme: &ResolvedTheme,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(root[0]);

    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[0]);

    if traces.is_empty() {
        render_empty_draw_state(frame, &root, &sidebar, &body, theme);
        return;
    }

    let active_trace = &traces[state.trace_index];

    render_entries_pane(frame, sidebar[0], active_trace, state, theme);
    render_traces_pane(frame, sidebar[1], traces, state, theme);
    render_diff_pane(frame, body[1], active_trace, state, theme);
    frame.render_widget(help_bar(theme), root[1]);
}

fn render_pane_scrollbar(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    content_length: usize,
    position: usize,
    viewport: usize,
    theme: &ResolvedTheme,
) {
    if content_length <= viewport.max(1) {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(scrollbar_symbols::VERTICAL)
        .thumb_symbol("\u{2590}")
        .track_style(Style::default().fg(to_color(theme.ui.border)))
        .thumb_style(Style::default().fg(to_color(theme.ui.border)))
        .begin_symbol(None)
        .end_symbol(None);
    // Ratatui puts the thumb at the track bottom only when position ==
    // content_length - 1.  Our scroll offset maxes out at content_length -
    // viewport, so pass max_scroll + 1 as the content length and clamp
    // position accordingly.  This makes the thumb reach the bottom for both
    // offset-based (diff) and selection-based (list) panes.
    let max_scroll = content_length.saturating_sub(viewport);
    let mut scrollbar_state = ScrollbarState::new(max_scroll.saturating_add(1))
        .position(position.min(max_scroll))
        .viewport_content_length(viewport);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scrollbar_state,
    );
}

fn pane_inner_height(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

fn pane_block(title: &'static str, color: Color) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
}

fn pane_block_with_footer(
    title: &'static str,
    color: Color,
    footer: Option<String>,
) -> Block<'static> {
    let mut block = pane_block(title, color);
    if let Some(footer) = footer {
        block = block.title_bottom(Line::from(footer).right_aligned());
    }
    block
}

fn position_footer(position: usize, total: usize) -> String {
    format!(" {position} of {total} ")
}

fn pane_border_color(active: bool, theme: &ResolvedTheme) -> Color {
    if active {
        to_color(theme.ui.border_active)
    } else {
        to_color(theme.ui.border)
    }
}

fn help_bar(theme: &ResolvedTheme) -> Paragraph<'static> {
    Paragraph::new("Tab/1/2/3 focus  j/k move  Shift+J/K or PgUp/PgDn scroll diff  q quit")
        .style(Style::default().fg(to_color(theme.ui.muted)))
}

fn entry_icon(ok: bool) -> (&'static str, Color) {
    if ok {
        ("\u{2713}", Color::Green)
    } else {
        ("\u{2717}", Color::Red)
    }
}

fn entry_label_line(entry: &HistoryEntry) -> Line<'static> {
    let (icon, color) = entry_icon(entry.ok);
    Line::from(vec![
        Span::styled(icon.to_string(), Style::default().fg(color)),
        Span::raw(format!(" {}", entry.summary)),
    ])
}

fn entry_label_plain(entry: &HistoryEntry) -> String {
    let (icon, _) = entry_icon(entry.ok);
    format!("{icon} {}", entry.summary)
}

fn trace_label(summary: &TraceSummary) -> String {
    let short_id = short_trace_id(&summary.trace_id);
    format!(
        "{}  {} entries  {}  {}",
        short_id, summary.entry_count, summary.last_timestamp, summary.last_summary
    )
}

fn short_trace_id(trace_id: &str) -> String {
    if trace_id.len() <= 10 {
        trace_id.to_string()
    } else {
        trace_id[..10].to_string()
    }
}

fn detail_lines(entry: &HistoryEntry) -> Vec<String> {
    if entry.ok {
        // v1 entries have empty hunks - show deprecation message
        if entry.hunks.is_empty() {
            return vec!["(old format, cannot display)".to_string()];
        }
        return detail_diff_lines_from_hunks(entry);
    } else if let Some(error) = &entry.error {
        return vec![format!("error: {error}")];
    }

    Vec::new()
}

fn detail_diff_lines_from_hunks(entry: &HistoryEntry) -> Vec<String> {
    use deltoids::LineKind;

    let mut result = Vec::new();
    let hunk_count = entry.hunks.len();
    let mut next_edit_index = 0usize;

    for (hunk_index, hunk) in entry.hunks.iter().enumerate() {
        // Add blank line between hunks
        if !result.is_empty() {
            result.push(String::new());
        }

        // Add edit summaries before the hunk
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

            let edit_slice = &entry.edits[next_edit_index..next_edit_index + edits_for_this_hunk];
            result.extend(
                edit_slice
                    .iter()
                    .map(|edit| format!("edit-summary: {}", edit.summary)),
            );
            next_edit_index += edits_for_this_hunk;
        }

        // Add the @@ header line
        let old_count = hunk
            .lines
            .iter()
            .filter(|l| l.kind != LineKind::Added)
            .count();
        let new_count = hunk
            .lines
            .iter()
            .filter(|l| l.kind != LineKind::Removed)
            .count();
        result.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, old_count, hunk.new_start, new_count
        ));

        // Add the diff lines
        for line in &hunk.lines {
            let prefix = match line.kind {
                LineKind::Added => '+',
                LineKind::Removed => '-',
                LineKind::Context => ' ',
            };
            result.push(format!("{}{}", prefix, line.content));
        }
    }

    result
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
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };

    let lines = detail_lines(entry);
    let mut rendered = render_detail_header(entry, width, theme);

    if !rendered.is_empty() && !lines.is_empty() {
        rendered.push(Line::from(""));
    }

    // Group lines into runs: subhunks (consecutive -/+ lines) and
    // non-subhunk lines (context, hunk headers, edit summaries, etc.).
    let groups = group_into_subhunks(&lines);

    for group in &groups {
        match group {
            LineGroup::Subhunk { start, end } => {
                render_subhunk(entry, &lines[*start..*end], width, &mut rendered, theme);
            }
            LineGroup::EditBlock(edit_lines) => {
                rendered.extend(render_edit_block(edit_lines, width, theme));
            }
            LineGroup::Other { index } => {
                rendered.extend(render_detail_line(entry, &lines[*index], width, theme));
            }
        }
    }

    rendered
}

/// A group of consecutive lines in the detail view.
#[derive(Debug, Clone)]
enum LineGroup {
    /// A subhunk: consecutive minus/plus diff lines.
    Subhunk { start: usize, end: usize },
    /// A block of consecutive edit-summary lines.
    EditBlock(Vec<String>),
    /// A single non-subhunk line (context, hunk header, etc.).
    Other { index: usize },
}

/// Group detail lines into subhunks and other lines.
///
/// A subhunk is a maximal run of lines starting with `-` or `+` (but not
/// `---` or `+++`). Edit-summary blocks are grouped together. Everything
/// else is a single `Other` entry.
fn group_into_subhunks(lines: &[String]) -> Vec<LineGroup> {
    let mut groups = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        if let Some((edit_lines, next_index)) = edit_block_markers(lines, index) {
            groups.push(LineGroup::EditBlock(edit_lines));
            index = next_index;
            continue;
        }

        if is_diff_change_line(&lines[index]) {
            let start = index;
            while index < lines.len() && is_diff_change_line(&lines[index]) {
                index += 1;
            }
            groups.push(LineGroup::Subhunk { start, end: index });
        } else {
            groups.push(LineGroup::Other { index });
            index += 1;
        }
    }

    groups
}

/// Check if a line is a diff change line (starts with `-` or `+`,
/// but not `---` or `+++`).
fn is_diff_change_line(line: &str) -> bool {
    (line.starts_with('-') && !line.starts_with("---"))
        || (line.starts_with('+') && !line.starts_with("+++"))
}

/// Render a subhunk with within-line emphasis.
///
/// Extracts minus and plus lines, computes emphasis via the intraline
/// module, and renders each line with the appropriate backgrounds.
fn render_subhunk(
    entry: &HistoryEntry,
    lines: &[String],
    width: usize,
    rendered: &mut Vec<Line<'static>>,
    theme: &ResolvedTheme,
) {
    // Separate minus and plus lines, preserving their order in the subhunk.
    let mut minus_contents: Vec<&str> = Vec::new();
    let mut plus_contents: Vec<&str> = Vec::new();
    let mut line_kinds: Vec<char> = Vec::new(); // '-' or '+'

    for line in lines {
        if let Some(content) = line.strip_prefix('-') {
            minus_contents.push(content);
            line_kinds.push('-');
        } else if let Some(content) = line.strip_prefix('+') {
            plus_contents.push(content);
            line_kinds.push('+');
        }
    }

    let (minus_emphasis, plus_emphasis) = compute_subhunk_emphasis(&minus_contents, &plus_contents);

    // Render in standard order (minus lines first, then plus lines, within
    // the subhunk). The lines are already in standard order from the diff.
    let mut mi = 0usize;
    let mut pi = 0usize;

    for kind in &line_kinds {
        match kind {
            '-' => {
                let content = minus_contents[mi];
                let emphasis = &minus_emphasis[mi];
                rendered.push(render_emphasized_line(
                    content,
                    emphasis,
                    to_color(theme.ui.diff_deleted_bg),
                    to_color(theme.ui.diff_deleted_emph_bg),
                    to_color(theme.ui.diff_deleted_bg),
                    &entry.path,
                    width,
                    theme,
                ));
                mi += 1;
            }
            '+' => {
                let content = plus_contents[pi];
                let emphasis = &plus_emphasis[pi];
                rendered.push(render_emphasized_line(
                    content,
                    emphasis,
                    to_color(theme.ui.diff_added_bg),
                    to_color(theme.ui.diff_added_emph_bg),
                    to_color(theme.ui.diff_added_bg),
                    &entry.path,
                    width,
                    theme,
                ));
                pi += 1;
            }
            _ => {}
        }
    }
}

/// Render a single diff line with emphasis information.
fn render_emphasized_line(
    content: &str,
    emphasis: &LineEmphasis,
    plain_bg: Color,
    emph_bg: Color,
    non_emph_bg: Color,
    path: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Line<'static> {
    match emphasis {
        LineEmphasis::Plain => syntax_diff_line(content, plain_bg, path, width, theme),
        LineEmphasis::Paired(sections) => {
            let bg_for_section = |section: &EmphSection| -> Color {
                match section.kind {
                    EmphKind::Emph => emph_bg,
                    EmphKind::NonEmph => non_emph_bg,
                }
            };
            let padding_bg = non_emph_bg;
            let (mut spans, visual_width) = highlighted_spans_with_emphasis(
                theme,
                path,
                content,
                sections,
                bg_for_section,
                width,
            );
            let padding = width.saturating_sub(visual_width);
            if padding > 0 {
                spans.push(Span::styled(
                    " ".repeat(padding),
                    Style::default().bg(padding_bg),
                ));
            }
            Line::from(spans)
        }
    }
}

fn render_detail_header(
    entry: &HistoryEntry,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    let path = collapse_home(&entry.path);
    let metadata = header_metadata_line(entry);
    render_header_block(&entry.summary, &path, &metadata, width, theme)
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

fn render_detail_line(
    entry: &HistoryEntry,
    line: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    if line.starts_with("edit-summary: ") {
        return vec![Line::from(Span::raw(line.to_string()))];
    }
    if let Some(rest) = line.strip_prefix("error: ") {
        return vec![labeled_line("error", rest, Color::Red)];
    }

    render_diff_line(entry, line, width, theme)
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

fn render_diff_line(
    entry: &HistoryEntry,
    line: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    if let Some(content) = line.strip_prefix('+').filter(|_| !line.starts_with("+++")) {
        return vec![syntax_diff_line(
            content,
            to_color(theme.ui.diff_added_bg),
            &entry.path,
            width,
            theme,
        )];
    }
    if let Some(content) = line.strip_prefix('-').filter(|_| !line.starts_with("---")) {
        return vec![syntax_diff_line(
            content,
            to_color(theme.ui.diff_deleted_bg),
            &entry.path,
            width,
            theme,
        )];
    }
    if let Some(content) = line.strip_prefix(' ') {
        return vec![syntax_diff_line(
            content,
            Color::Reset,
            &entry.path,
            width,
            theme,
        )];
    }
    if line.starts_with("@@") {
        // Look up structural scopes for this hunk from entry.hunks.
        let ancestors = parse_hunk_new_start(line)
            .and_then(|new_start| {
                entry
                    .hunks
                    .iter()
                    .find(|h| h.new_start == new_start)
                    .map(|h| h.ancestors.as_slice())
            })
            .unwrap_or(&[]);
        return render_hunk_separator(line, &entry.path, width, Some(ancestors), theme);
    }

    vec![Line::from(Span::raw(fit_line(line, width)))]
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
    summary: &str,
    path: &str,
    metadata: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    let summary_style = Style::default()
        .fg(to_color(theme.ui.border_active))
        .add_modifier(Modifier::BOLD);

    if width < 4 {
        return vec![Line::from(Span::styled(
            fit_line(summary, width),
            summary_style,
        ))];
    }

    let path_style = Style::default().fg(to_color(theme.ui.border));
    let metadata_style = Style::default().fg(to_color(theme.ui.muted));
    let border = Style::default().fg(to_color(theme.ui.border));
    let bot = format!("─{}", "─".repeat(width.saturating_sub(1)));

    let mut lines = Vec::new();
    for wrapped in wrap_text(summary, width) {
        lines.push(Line::from(Span::styled(wrapped, summary_style)));
    }
    lines.push(Line::from(Span::styled(fit_line(path, width), path_style)));
    lines.push(Line::from(Span::styled(
        fit_line(metadata, width),
        metadata_style,
    )));
    lines.push(Line::from(Span::styled(bot, border)));
    lines
}

fn edit_block_markers(lines: &[String], start: usize) -> Option<(Vec<String>, usize)> {
    if start >= lines.len() {
        return None;
    }

    let first = lines[start].strip_prefix("edit-summary: ")?;

    let mut items = vec![first.to_string()];
    let mut index = start + 1;
    while index < lines.len() {
        let Some(rest) = lines[index].strip_prefix("edit-summary: ") else {
            break;
        };
        items.push(rest.to_string());
        index += 1;
    }

    Some((items, index))
}

fn render_edit_block(lines: &[String], width: usize, theme: &ResolvedTheme) -> Vec<Line<'static>> {
    let border = Style::default().fg(to_color(theme.ui.border_active));
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

fn syntax_diff_line(
    content: &str,
    bg: Color,
    path: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Line<'static> {
    let base_style = Style::default().bg(bg);

    let (mut spans, visual_width) = highlighted_spans(theme, path, content, base_style, width);
    let padding = width.saturating_sub(visual_width);
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), base_style));
    }

    Line::from(spans)
}

/// Parse the new-file start line from a diff hunk header.
/// Input: `@@ -74,15 +75,14 @@` -> Some(75)
fn parse_hunk_new_start(line: &str) -> Option<usize> {
    let after_plus = line.find('+').map(|i| &line[i + 1..])?;
    let end = after_plus.find([',', ' '])?;
    after_plus[..end].parse().ok()
}

fn render_hunk_separator(
    line: &str,
    path: &str,
    width: usize,
    ancestors: Option<&[deltoids::ScopeNode]>,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    // If we have structural scope ancestors, render the multi-line breadcrumb box.
    if let Some(ancestors) = ancestors
        && !ancestors.is_empty()
    {
        return render_breadcrumb_box(ancestors, path, width, theme);
    }

    // Fall back to the legacy single-line rendering.
    render_hunk_separator_legacy(line, path, width, theme)
}

fn render_breadcrumb_box(
    ancestors: &[deltoids::ScopeNode],
    path: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    let border = Style::default().fg(to_color(theme.ui.border));
    let max_content_width = width.saturating_sub(2); // room for " │"

    // Compute the widest line number for right-alignment.
    let max_line_num = ancestors.iter().map(|a| a.start_line).max().unwrap_or(0);
    let num_col_width = max_line_num.to_string().len();

    // Build content rows: ancestor lines with optional "..." gaps between.
    struct Row {
        line_num: Option<usize>,
        text: Option<String>, // None for "..." rows
    }
    let mut rows: Vec<Row> = Vec::new();
    for (i, ancestor) in ancestors.iter().enumerate() {
        if i > 0 {
            let prev = &ancestors[i - 1];
            if prev.start_line + 1 < ancestor.start_line {
                rows.push(Row {
                    line_num: None,
                    text: None,
                });
            }
        }
        rows.push(Row {
            line_num: Some(ancestor.start_line),
            text: Some(ancestor.text.clone()),
        });
    }

    // Compute the widest rendered line for box width.
    let prefix_width = num_col_width + 2; // "NNN: "
    let mut max_row_width = 0usize;
    for row in &rows {
        let row_width = match &row.text {
            Some(text) => prefix_width + display_width(text),
            None => prefix_width + 3, // "..."
        };
        max_row_width = max_row_width.max(row_width);
    }
    let content_width = max_row_width.min(max_content_width);

    let top = format!("{}╮", "─".repeat(content_width + 1));
    let bot = format!("{}╯", "─".repeat(content_width + 1));

    let mut lines = vec![Line::from(Span::styled(top, border))];

    for row in &rows {
        match &row.text {
            Some(text) => {
                let num_str = format!(
                    "{:>width$}: ",
                    row.line_num.unwrap_or(0),
                    width = num_col_width
                );
                let available_text_width = content_width.saturating_sub(prefix_width);
                let (mut code_spans, code_width) = highlighted_spans(
                    theme,
                    path,
                    text,
                    Style::default(),
                    available_text_width.max(1),
                );
                let padding = content_width.saturating_sub(prefix_width + code_width);

                let mut spans = vec![Span::styled(num_str, border)];
                spans.append(&mut code_spans);
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::styled(" │", border));
                lines.push(Line::from(spans));
            }
            None => {
                // "..." gap row
                let dots = format!("{:>width$}  ...", "", width = num_col_width);
                let padding = content_width.saturating_sub(display_width(&dots));
                let mut spans = vec![Span::styled(dots, border)];
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::styled(" │", border));
                lines.push(Line::from(spans));
            }
        }
    }

    lines.push(Line::from(Span::styled(bot, border)));
    lines
}

fn render_hunk_separator_legacy(
    line: &str,
    path: &str,
    width: usize,
    theme: &ResolvedTheme,
) -> Vec<Line<'static>> {
    // Parse optional function context from `@@ -N,M +N,M @@ context`.
    let context = line
        .find("@@ ")
        .and_then(|start| {
            let rest = &line[start + 3..];
            rest.find("@@").map(|end| rest[end + 2..].trim())
        })
        .unwrap_or("");

    let line_number = parse_hunk_new_start(line);
    let border = Style::default().fg(to_color(theme.ui.border));

    let prefix = match line_number {
        Some(n) => format!("{n}: "),
        None => String::new(),
    };
    let prefix_width = display_width(&prefix);
    let max_label_width = width.saturating_sub(2);

    if context.is_empty() {
        let label = match line_number {
            Some(n) => n.to_string(),
            None => return vec![Line::from("")],
        };
        let label_width = display_width(&label);
        let top = format!("{}╮", "─".repeat(label_width + 1));
        let mid = format!("{label} │");
        let bot = format!("{}╯", "─".repeat(label_width + 1));
        return vec![
            Line::from(Span::styled(top, border)),
            Line::from(Span::styled(mid, border)),
            Line::from(Span::styled(bot, border)),
        ];
    }

    let available_context_width = max_label_width.saturating_sub(prefix_width);
    let (mut code_spans, code_width) = highlighted_spans(
        theme,
        path,
        context,
        Style::default(),
        available_context_width.max(1),
    );
    let label_width = prefix_width + code_width;
    let top = format!("{}╮", "─".repeat(label_width + 1));
    let bot = format!("{}╯", "─".repeat(label_width + 1));

    let mut mid_spans = Vec::new();
    if !prefix.is_empty() {
        mid_spans.push(Span::styled(prefix, border));
    }
    mid_spans.append(&mut code_spans);
    mid_spans.push(Span::styled(" │", border));

    vec![
        Line::from(Span::styled(top, border)),
        Line::from(mid_spans),
        Line::from(Span::styled(bot, border)),
    ]
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

fn run_scripted(traces: &[LoadedTrace], theme: &ResolvedTheme) -> Result<(), String> {
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
    theme: &ResolvedTheme,
) -> String {
    if traces.is_empty() {
        return "No traces found for this directory.\n".to_string();
    }

    let left_width = (width / 3).max(30).min(width.saturating_sub(20));
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

    fn test_theme() -> ResolvedTheme {
        ResolvedTheme::resolve()
    }

    fn edit_entry() -> HistoryEntry {
        HistoryEntry {
            v: 1,
            tool: "edit".to_string(),
            trace_id: "01JTESTTRACE00000000000000".to_string(),
            timestamp: "2026-04-16T12:00:00Z".to_string(),
            cwd: "/tmp/project".to_string(),
            path: "/tmp/project/app.txt".to_string(),
            summary: "Update x constant".to_string(),
            ok: true,
            edits: vec![TextEdit {
                summary: "Edit change".to_string(),
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
            summary: "Rewrite config".to_string(),
            ok: true,
            edits: Vec::new(),
            content: "{\n  \"version\": 2\n}\n".to_string(),
            diff: Some(
                "--- a/config.json\n+++ b/config.json\n@@ -1,3 +1,3 @@\n   \"version\": 1\n+  \"version\": 2\n"
                    .to_string(),
            ),
            error: None,
            hunks: Vec::new(),
        }
    }

    fn trace_summary(trace_id: &str, entry_count: usize, last_summary: &str) -> TraceSummary {
        TraceSummary {
            trace_id: trace_id.to_string(),
            entry_count,
            last_timestamp: "2026-04-16T12:00:00Z".to_string(),
            last_tool: "edit".to_string(),
            last_path: "/tmp/project/app.txt".to_string(),
            last_summary: last_summary.to_string(),
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

        assert!(output.contains("\u{2713} Update x constant"));
        assert!(output.contains("\u{2713} Rewrite config"));
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
    fn detail_lines_renders_from_hunks() {
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

        let lines = detail_lines(&entry);

        // Should have hunk separator + diff lines
        assert!(
            lines.iter().any(|l: &String| l.starts_with("@@")),
            "should have hunk header"
        );
        assert!(
            lines.iter().any(|l: &String| l == " context line"),
            "should have context line"
        );
        assert!(
            lines.iter().any(|l: &String| l == "-old line"),
            "should have removed line"
        );
        assert!(
            lines.iter().any(|l: &String| l == "+new line"),
            "should have added line"
        );
    }

    #[test]
    fn detail_lines_shows_deprecation_for_v1_entry() {
        let entry = edit_entry(); // v1 entry with empty hunks

        let lines = detail_lines(&entry);

        assert_eq!(lines, vec!["(old format, cannot display)".to_string()]);
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

        assert!(output.contains("> \u{2713} Rewrite config"));
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
    fn parse_hunk_new_start_extracts_line_number() {
        assert_eq!(parse_hunk_new_start("@@ -74,15 +75,14 @@"), Some(75));
        assert_eq!(parse_hunk_new_start("@@ -1 +1,3 @@"), Some(1));
        assert_eq!(parse_hunk_new_start("@@ -10,5 +20 @@"), Some(20));
        assert_eq!(parse_hunk_new_start("not a hunk"), None);
    }

    #[test]
    fn render_detail_header_uses_summary_path_metadata_and_rule() {
        let theme = test_theme();
        let lines = render_detail_header(&edit_entry(), 80, &theme);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].to_string().starts_with("Update x constant"));
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(to_color(theme.ui.border_active))
        );
        assert!(lines[1].to_string().starts_with("/tmp/project/app.txt"));
        assert_eq!(lines[1].spans[0].style.fg, Some(to_color(theme.ui.border)));
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
    fn render_detail_header_wraps_long_summary() {
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.summary = "This is a long summary that should wrap onto multiple lines".to_string();
        let lines = render_detail_header(&entry, 30, &theme);
        // Summary wraps into multiple lines, then path, metadata, rule.
        assert!(
            lines.len() > 4,
            "long summary should produce more than 4 lines, got {}",
            lines.len()
        );
        // All summary lines are border_active (orange) bold.
        let rule_index = lines
            .iter()
            .position(|l| l.to_string().starts_with('─'))
            .expect("should have a bottom rule");
        for line in &lines[..rule_index - 2] {
            assert_eq!(
                line.spans[0].style.fg,
                Some(to_color(theme.ui.border_active)),
                "wrapped summary line should be border_active color"
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
        let lines = render_edit_block(&["Rename renderer".to_string()], 80, &theme);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].to_string().starts_with('─'));
        assert!(lines[1].to_string().starts_with("Rename renderer"));
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(to_color(theme.ui.border_active))
        );
        assert!(lines[2].to_string().ends_with('╯'));
    }

    #[test]
    fn hunk_separator_renders_boxed_label_with_line_number_and_context() {
        let theme = test_theme();
        let lines = render_hunk_separator(
            "@@ -10,5 +42,6 @@ fn hello() {",
            "src/lib.rs",
            80,
            None,
            &theme,
        );
        assert_eq!(lines.len(), 3, "expected 3 lines for box");

        let top = lines[0].to_string();
        let mid = lines[1].to_string();
        let bot = lines[2].to_string();

        assert!(top.starts_with('─'), "top should start with ─");
        assert!(top.ends_with('\u{256e}'), "top should end with ╮");
        assert!(
            mid.starts_with("42: fn hello() {"),
            "mid should contain line number and context"
        );
        assert!(mid.ends_with('│'), "mid should end with │");
        assert!(
            lines[1].spans.len() >= 2,
            "mid line should be split into styled spans"
        );
        assert!(bot.starts_with('─'), "bot should start with ─");
        assert!(bot.ends_with('\u{256f}'), "bot should end with ╯");
    }

    #[test]
    fn hunk_separator_renders_line_number_only_when_no_context() {
        let theme = test_theme();
        let lines = render_hunk_separator("@@ -1,3 +7,3 @@", "src/lib.rs", 80, None, &theme);
        assert_eq!(lines.len(), 3);
        let mid = lines[1].to_string();
        assert!(mid.contains("7"), "mid should contain line number");
        assert!(!mid.contains(':'), "no colon when there is no context");
    }

    #[test]
    fn hunk_separator_renders_empty_for_no_info() {
        let theme = test_theme();
        // Malformed line with no parseable info.
        let lines = render_hunk_separator("@@ @@", "src/lib.rs", 80, None, &theme);
        assert_eq!(lines.len(), 1, "should fall back to single empty line");
    }

    #[test]
    fn group_into_subhunks_groups_change_lines() {
        let lines: Vec<String> = vec![
            "@@ -1,3 +1,3 @@".into(),
            "-old line 1".into(),
            "-old line 2".into(),
            "+new line 1".into(),
            "+new line 2".into(),
            " context".into(),
        ];
        let groups = group_into_subhunks(&lines);
        assert_eq!(groups.len(), 3); // hunk header, subhunk, context
        assert!(matches!(groups[0], LineGroup::Other { index: 0 }));
        assert!(matches!(groups[1], LineGroup::Subhunk { start: 1, end: 5 }));
        assert!(matches!(groups[2], LineGroup::Other { index: 5 }));
    }

    #[test]
    fn group_into_subhunks_splits_on_context_lines() {
        let lines: Vec<String> = vec![
            "-old1".into(),
            "+new1".into(),
            " context".into(),
            "-old2".into(),
            "+new2".into(),
        ];
        let groups = group_into_subhunks(&lines);
        assert_eq!(groups.len(), 3);
        assert!(matches!(groups[0], LineGroup::Subhunk { start: 0, end: 2 }));
        assert!(matches!(groups[1], LineGroup::Other { index: 2 }));
        assert!(matches!(groups[2], LineGroup::Subhunk { start: 3, end: 5 }));
    }

    #[test]
    fn group_into_subhunks_handles_edit_summaries() {
        let lines: Vec<String> = vec![
            "edit-summary: first".into(),
            "edit-summary: second".into(),
            "@@ -1 +1 @@".into(),
            "-old".into(),
            "+new".into(),
        ];
        let groups = group_into_subhunks(&lines);
        assert!(matches!(groups[0], LineGroup::EditBlock(_)));
        assert!(matches!(groups[1], LineGroup::Other { index: 2 }));
        assert!(matches!(groups[2], LineGroup::Subhunk { start: 3, end: 5 }));
    }

    #[test]
    fn is_diff_change_line_recognizes_minus_and_plus() {
        assert!(is_diff_change_line("-old line"));
        assert!(is_diff_change_line("+new line"));
        assert!(!is_diff_change_line("--- a/file"));
        assert!(!is_diff_change_line("+++ b/file"));
        assert!(!is_diff_change_line(" context"));
        assert!(!is_diff_change_line("@@ -1 +1 @@"));
    }

    #[test]
    fn render_emphasized_line_plain_uses_flat_bg() {
        let theme = test_theme();
        let line = render_emphasized_line(
            "const x = 1;",
            &LineEmphasis::Plain,
            to_color(theme.ui.diff_deleted_bg),
            to_color(theme.ui.diff_deleted_emph_bg),
            to_color(theme.ui.diff_deleted_bg),
            "test.rs",
            80,
            &theme,
        );
        // All spans should have the plain deleted background.
        for span in &line.spans {
            assert_eq!(
                span.style.bg,
                Some(to_color(theme.ui.diff_deleted_bg)),
                "plain line should use flat bg, got {:?}",
                span.style.bg
            );
        }
    }

    #[test]
    fn render_emphasized_line_paired_uses_emph_and_non_emph_bg() {
        use deltoids::{EmphKind, EmphSection};
        let theme = test_theme();
        let sections = vec![
            EmphSection {
                kind: EmphKind::NonEmph,
                text: "const x = ".to_string(),
            },
            EmphSection {
                kind: EmphKind::Emph,
                text: "1".to_string(),
            },
            EmphSection {
                kind: EmphKind::NonEmph,
                text: ";".to_string(),
            },
        ];
        let line = render_emphasized_line(
            "const x = 1;",
            &LineEmphasis::Paired(sections),
            to_color(theme.ui.diff_deleted_bg),
            to_color(theme.ui.diff_deleted_emph_bg),
            to_color(theme.ui.diff_deleted_bg),
            "test.rs",
            80,
            &theme,
        );
        // Should have at least one span with emph bg and one with non-emph bg.
        let has_emph_bg = line
            .spans
            .iter()
            .any(|s| s.style.bg == Some(to_color(theme.ui.diff_deleted_emph_bg)));
        let has_non_emph_bg = line
            .spans
            .iter()
            .any(|s| s.style.bg == Some(to_color(theme.ui.diff_deleted_bg)));
        assert!(has_emph_bg, "paired line should have emph bg spans");
        assert!(has_non_emph_bg, "paired line should have non-emph bg spans");
    }

    #[test]
    fn render_subhunk_pairs_similar_lines() {
        let theme = test_theme();
        let entry = edit_entry();
        let lines = vec!["-const x = 1;".to_string(), "+const x = 2;".to_string()];
        let mut rendered = Vec::new();
        render_subhunk(&entry, &lines, 80, &mut rendered, &theme);
        assert_eq!(rendered.len(), 2);

        // Paired minus line should have emph bg on the changed token.
        let minus_has_emph = rendered[0]
            .spans
            .iter()
            .any(|s| s.style.bg == Some(to_color(theme.ui.diff_deleted_emph_bg)));
        assert!(
            minus_has_emph,
            "paired minus line should have emph bg on changed token"
        );

        // Paired plus line should have emph bg on the changed token.
        let plus_has_emph = rendered[1]
            .spans
            .iter()
            .any(|s| s.style.bg == Some(to_color(theme.ui.diff_added_emph_bg)));
        assert!(
            plus_has_emph,
            "paired plus line should have emph bg on changed token"
        );
    }

    #[test]
    fn render_subhunk_leaves_dissimilar_lines_plain() {
        let theme = test_theme();
        let entry = edit_entry();
        let lines = vec![
            "-aaa bbb ccc ddd eee".to_string(),
            "+xxx yyy zzz www qqq".to_string(),
        ];
        let mut rendered = Vec::new();
        render_subhunk(&entry, &lines, 80, &mut rendered, &theme);
        assert_eq!(rendered.len(), 2);

        // Dissimilar lines should use the plain background.
        let minus_has_plain = rendered[0]
            .spans
            .iter()
            .all(|s| s.style.bg == Some(to_color(theme.ui.diff_deleted_bg)));
        assert!(minus_has_plain, "unpaired minus should use plain bg");

        let plus_has_plain = rendered[1]
            .spans
            .iter()
            .all(|s| s.style.bg == Some(to_color(theme.ui.diff_added_bg)));
        assert!(plus_has_plain, "unpaired plus should use plain bg");
    }

    // -----------------------------------------------------------------------
    // Breadcrumb box tests
    // -----------------------------------------------------------------------

    fn scope_node(
        kind: &str,
        name: &str,
        start: usize,
        end: usize,
        text: &str,
    ) -> deltoids::ScopeNode {
        deltoids::ScopeNode {
            kind: kind.to_string(),
            name: name.to_string(),
            start_line: start,
            end_line: end,
            text: text.to_string(),
        }
    }

    #[test]
    fn breadcrumb_box_renders_multi_ancestor_with_line_numbers() {
        let theme = test_theme();
        let ancestors = vec![
            scope_node("impl_item", "Foo", 3, 50, "impl Foo {"),
            scope_node(
                "function_item",
                "compute",
                75,
                80,
                "    fn compute(&self) -> i32 {",
            ),
        ];
        let lines = render_hunk_separator(
            "@@ -74,7 +75,7 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // top border + 2 ancestor lines + dots row + bottom border = 5
        assert_eq!(
            lines.len(),
            5,
            "expected 5 lines for breadcrumb box with gap"
        );

        let top = lines[0].to_string();
        assert!(top.starts_with('\u{2500}'), "top starts with ─");
        assert!(top.ends_with('\u{256e}'), "top ends with ╮");

        // First ancestor line should contain " 3: impl Foo {"
        let mid1 = lines[1].to_string();
        assert!(mid1.contains(" 3:"), "first ancestor should show line 3");
        assert!(
            mid1.contains("impl Foo"),
            "first ancestor should show impl Foo"
        );
        assert!(mid1.ends_with('│'), "ancestor line ends with │");

        // Dots line
        let dots = lines[2].to_string();
        assert!(dots.contains("..."), "gap marker should contain ...");

        // Second ancestor line should contain "75:"
        let mid2 = lines[3].to_string();
        assert!(mid2.contains("75:"), "second ancestor should show line 75");
        assert!(
            mid2.contains("fn compute"),
            "second ancestor should show fn compute"
        );

        let bot = lines[4].to_string();
        assert!(bot.ends_with('\u{256f}'), "bot ends with ╯");
    }

    #[test]
    fn breadcrumb_box_no_dots_for_adjacent_ancestors() {
        let theme = test_theme();
        let ancestors = vec![
            scope_node("impl_item", "Foo", 3, 50, "impl Foo {"),
            scope_node("function_item", "new", 4, 10, "    fn new() -> Self {"),
        ];
        let lines = render_hunk_separator(
            "@@ -4,3 +4,3 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // top + 2 ancestors + bottom = 4 (no dots because end_line+1 >= next.start_line)
        assert_eq!(lines.len(), 4, "expected 4 lines for adjacent ancestors");
        // No dots line
        for line in &lines {
            assert!(
                !line.to_string().contains("..."),
                "no dots for adjacent scopes"
            );
        }
    }

    #[test]
    fn breadcrumb_box_single_ancestor() {
        let theme = test_theme();
        let ancestors = vec![scope_node(
            "function_item",
            "bar",
            6,
            10,
            "fn bar(a: i32, b: i32) -> i32 {",
        )];
        let lines = render_hunk_separator(
            "@@ -6,3 +6,3 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // top + 1 ancestor + bottom = 3
        assert_eq!(lines.len(), 3);
        let mid = lines[1].to_string();
        assert!(mid.contains("6:"), "should show line number");
        assert!(mid.contains("fn bar"), "should show function");
    }

    #[test]
    fn breadcrumb_box_empty_ancestors_falls_back_to_legacy() {
        let theme = test_theme();
        // Empty ancestors should fall back to legacy @@ parsing
        let lines = render_hunk_separator(
            "@@ -1,3 +7,3 @@ fn hello() {",
            "src/lib.rs",
            80,
            Some(&[]),
            &theme,
        );
        // Legacy rendering: top + mid + bot = 3
        assert_eq!(lines.len(), 3);
        let mid = lines[1].to_string();
        assert!(mid.contains("7:"), "legacy: line number from @@");
        assert!(mid.contains("fn hello"), "legacy: context from @@");
    }

    #[test]
    fn breadcrumb_box_line_numbers_right_aligned() {
        let theme = test_theme();
        let ancestors = vec![
            scope_node("impl_item", "Server", 1, 200, "impl Server {"),
            scope_node(
                "function_item",
                "handle",
                120,
                150,
                "    if let Some(val) = body {",
            ),
        ];
        let lines = render_hunk_separator(
            "@@ -120,3 +120,3 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // Line numbers should be right-aligned: "  1:" and "120:"
        let mid1 = lines[1].to_string();
        let mid2 = lines[3].to_string(); // after dots row
        assert!(
            mid1.starts_with("  1:"),
            "line 1 should be padded to 3 digits, got: {mid1:?}"
        );
        assert!(
            mid2.starts_with("120:"),
            "line 120 should take full width, got: {mid2:?}"
        );
    }

    #[test]
    fn breadcrumb_box_deep_nesting() {
        let theme = test_theme();
        let ancestors = vec![
            scope_node("impl_item", "Server", 1, 200, "impl Server {"),
            scope_node(
                "function_item",
                "handle",
                45,
                180,
                "    fn handle(&self, req: Request) {",
            ),
            scope_node(
                "function_item",
                "body",
                120,
                140,
                "        if let Some(val) = body {",
            ),
        ];
        let lines = render_hunk_separator(
            "@@ -120,3 +120,3 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // top + ancestor1 + dots + ancestor2 + dots + ancestor3 + bottom = 7
        assert_eq!(
            lines.len(),
            7,
            "expected 7 lines for deep nesting with two gaps"
        );
    }

    // -----------------------------------------------------------------------
    // Breadcrumb redundancy detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn breadcrumb_shows_all_ancestors() {
        let theme = test_theme();
        // All ancestors are shown regardless of visibility in diff
        let ancestors = vec![
            scope_node("impl_item", "Foo", 3, 50, "impl Foo {"),
            scope_node(
                "function_item",
                "compute",
                10,
                20,
                "    fn compute(&self) {",
            ),
        ];
        let lines = render_hunk_separator(
            "@@ -10,3 +10,3 @@",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // Should show both: top + impl + dots + fn + bottom = 5
        assert_eq!(lines.len(), 5, "should show all ancestors");
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("impl Foo"), "should show outer");
        assert!(combined.contains("fn compute"), "should show innermost");
    }

    #[test]
    fn breadcrumb_shows_single_ancestor() {
        let theme = test_theme();
        // Single ancestor is always shown
        let ancestors = vec![scope_node("function_item", "bar", 6, 10, "fn bar() {")];
        let lines = render_hunk_separator(
            "@@ -6,3 +6,3 @@ fn bar() {",
            "src/lib.rs",
            80,
            Some(&ancestors),
            &theme,
        );
        // Should show ancestor: top + mid + bottom = 3
        assert_eq!(lines.len(), 3);
        let mid = lines[1].to_string();
        assert!(mid.contains("6:"), "shows line number");
        assert!(mid.contains("fn bar"), "shows ancestor");
    }
}
