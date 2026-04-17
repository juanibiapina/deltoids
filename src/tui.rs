//! Terminal UI for browsing edit/write traces for the current directory.
//!
//! Layout (lazygit-inspired):
//! - Left sidebar, top: entries (edits/writes) of the selected trace.
//! - Left sidebar, bottom: traces for the current working directory.
//! - Right: diff / detail for the selected entry.
//!
//! Focus toggles between the traces pane and the entries pane with `Tab`.

use std::io::{self, IsTerminal, Read};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
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

use crate::highlight::highlighted_spans;
use crate::{HistoryEntry, TraceSummary, list_traces_for_current_directory, read_history_entries};

const TOKYONIGHT_ORANGE: Color = Color::Rgb(255, 150, 108);
const TOKYONIGHT_BLUE: Color = Color::Rgb(122, 162, 247);
const TOKYONIGHT_CYAN: Color = Color::Rgb(101, 188, 255);
const TOKYONIGHT_DIM: Color = Color::Rgb(86, 95, 137);
const DIFF_ADDED_BG: Color = Color::Rgb(29, 43, 52);
const DIFF_DELETED_BG: Color = Color::Rgb(48, 31, 39);
const SELECTION_BG: Color = Color::Rgb(45, 63, 118);

/// Entry point. Loads traces for the current directory and opens the TUI
/// (or renders a scripted view when stdout is not a terminal).
pub fn run() -> Result<(), String> {
    let cwd = current_cwd_or_empty();
    let traces = list_traces_for_current_directory()?;
    let mut loaded = Vec::with_capacity(traces.len());
    for trace in traces {
        let entries = read_history_entries(&trace.trace_id)?
            .into_iter()
            .filter(|entry| entry.cwd == cwd)
            .collect::<Vec<_>>();
        loaded.push(LoadedTrace { trace, entries });
    }

    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        run_tui(&loaded)
    } else {
        run_scripted(&loaded)
    }
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
}

impl AppState {
    fn new(trace_count: usize) -> Self {
        Self {
            focus: Focus::Entries,
            trace_index: 0,
            entry_indices: vec![0; trace_count],
            diff_scroll: 0,
            diff_cache: None,
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
        KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
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
        KeyCode::Char('j') | KeyCode::Down => {
            if state.focus == Focus::Diff {
                let max_scroll = max_detail_scroll(detail_row_count, detail_height);
                state.diff_scroll = (state.diff_scroll + 1).min(max_scroll);
            } else {
                move_down(state, traces);
            }
            AppCommand::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.focus == Focus::Diff {
                state.diff_scroll = state.diff_scroll.saturating_sub(1);
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

fn run_tui(traces: &[LoadedTrace]) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("Failed to create screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

    let mut state = AppState::new(traces.len());

    loop {
        let (detail_row_count, detail_height) = terminal
            .draw(|frame| draw(frame, traces, &mut state))
            .map(|completed| {
                let detail_row_count = detail_rows(traces, &state).len();
                let height = completed.area.height.saturating_sub(3) as usize;
                (detail_row_count, height)
            })
            .map_err(|err| format!("Failed to render screen: {err}"))?;

        match event::read().map_err(|err| format!("Failed to read input event: {err}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(
                    &mut state,
                    traces,
                    key.code,
                    detail_row_count,
                    detail_height,
                ) == AppCommand::Quit
                {
                    break;
                }
            }
            _ => {}
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

fn draw(frame: &mut ratatui::Frame<'_>, traces: &[LoadedTrace], state: &mut AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(root[0]);

    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[0]);

    if traces.is_empty() {
        let message = Paragraph::new("No traces found for this directory.")
            .style(Style::default().fg(TOKYONIGHT_DIM))
            .block(pane_block_with_footer(
                " [1] Entries ",
                TOKYONIGHT_BLUE,
                Some(position_footer(0, 0)),
            ));
        frame.render_widget(message, sidebar[0]);
        frame.render_widget(
            pane_block_with_footer(
                " [2] Traces ",
                TOKYONIGHT_BLUE,
                Some(position_footer(0, 0)),
            ),
            sidebar[1],
        );
        frame.render_widget(pane_block(" [3] Diff ", TOKYONIGHT_BLUE), body[1]);
        frame.render_widget(help_bar(), root[1]);
        return;
    }

    let active_trace = &traces[state.trace_index];

    // Entries pane (top-left)
    let entry_items = active_trace
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_label(entry)))
        .collect::<Vec<_>>();
    let entries_count = active_trace.entries.len();
    let entries_position = if entries_count == 0 {
        0
    } else {
        state.entry_index() + 1
    };
    let mut entries_state = ListState::default().with_selected(Some(state.entry_index()));
    let entries_list = List::new(entry_items)
        .block(pane_block_with_footer(
            " [1] Entries ",
            pane_border_color(state.focus == Focus::Entries, TOKYONIGHT_ORANGE),
            Some(position_footer(entries_position, entries_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(entries_list, sidebar[0], &mut entries_state);

    // Traces pane (bottom-left)
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
    let mut traces_state = ListState::default().with_selected(Some(state.trace_index));
    let traces_list = List::new(trace_items)
        .block(pane_block_with_footer(
            " [2] Traces ",
            pane_border_color(state.focus == Focus::Traces, TOKYONIGHT_ORANGE),
            Some(position_footer(traces_position, traces_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(traces_list, sidebar[1], &mut traces_state);

    // Diff pane (right, full height). Render into the cache when the
    // selection or available width changes; otherwise reuse the previous
    // frame's lines so scroll keys stay snappy on large diffs.
    let detail_width = body[1].width.saturating_sub(2) as usize;
    let entry_index = state.entry_index();
    let cache_valid = state
        .diff_cache
        .as_ref()
        .is_some_and(|cache| {
            cache.trace_index == state.trace_index
                && cache.entry_index == entry_index
                && cache.width == detail_width
        });
    if !cache_valid {
        let lines = render_detail_for(active_trace, entry_index, detail_width);
        state.diff_cache = Some(DiffCache {
            trace_index: state.trace_index,
            entry_index,
            width: detail_width,
            lines,
        });
    }
    let diff_viewport = body[1].height.saturating_sub(2) as usize;
    let detail_row_count = state
        .diff_cache
        .as_ref()
        .map(|cache| cache.lines.len())
        .unwrap_or(0);
    let start = state.diff_scroll.min(detail_row_count);
    let end = start.saturating_add(diff_viewport.max(1)).min(detail_row_count);
    let visible_lines: Vec<Line<'static>> = state
        .diff_cache
        .as_ref()
        .map(|cache| cache.lines[start..end].to_vec())
        .unwrap_or_default();
    let diff = Paragraph::new(visible_lines).block(pane_block(
        " [3] Diff ",
        pane_border_color(state.focus == Focus::Diff, TOKYONIGHT_ORANGE),
    ));
    frame.render_widget(diff, body[1]);

    if detail_row_count > diff_viewport {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(scrollbar_symbols::VERTICAL)
            .thumb_symbol("\u{2590}")
            .track_style(Style::default().fg(TOKYONIGHT_BLUE))
            .thumb_style(Style::default().fg(TOKYONIGHT_BLUE))
            .begin_symbol(None)
            .end_symbol(None);
        let mut scrollbar_state =
            ScrollbarState::new(detail_row_count).position(state.diff_scroll);
        frame.render_stateful_widget(
            scrollbar,
            body[1].inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    frame.render_widget(help_bar(), root[1]);
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

fn pane_border_color(active: bool, active_color: Color) -> Color {
    if active { active_color } else { TOKYONIGHT_BLUE }
}

fn help_bar() -> Paragraph<'static> {
    Paragraph::new("Tab/1/2/3 focus  j/k move  PgUp/PgDn scroll  q quit")
        .style(Style::default().fg(Color::DarkGray))
}

fn entry_label(entry: &HistoryEntry) -> String {
    let status = if entry.ok { "ok" } else { "fail" };
    format!("{} {} {}", entry.tool, status, entry.summary)
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

fn detail_rows(traces: &[LoadedTrace], state: &AppState) -> Vec<String> {
    let Some(trace) = traces.get(state.trace_index) else {
        return Vec::new();
    };
    let Some(entry) = trace.entries.get(state.entry_index()) else {
        return Vec::new();
    };
    detail_lines(entry)
}

fn detail_lines(entry: &HistoryEntry) -> Vec<String> {
    let mut lines = vec![
        format!("tool: {}", entry.tool),
        format!("summary: {}", entry.summary),
        format!("path: {}", entry.path),
    ];

    if entry.ok {
        lines.push("diff:".to_string());
        if let Some(diff) = &entry.diff {
            lines.extend(diff.lines().map(str::to_string));
        }
    } else if let Some(error) = &entry.error {
        lines.push(format!("error: {error}"));
    }

    lines
}

fn render_detail_for(trace: &LoadedTrace, entry_index: usize, width: usize) -> Vec<Line<'static>> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };
    detail_lines(entry)
        .iter()
        .map(|line| render_detail_line(entry, line, width))
        .collect()
}

fn render_detail_line(entry: &HistoryEntry, line: &str, width: usize) -> Line<'static> {
    if line == "diff:" {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(TOKYONIGHT_CYAN),
        ));
    }

    if let Some(rest) = line.strip_prefix("tool: ") {
        return labeled_line("tool", rest, TOKYONIGHT_CYAN);
    }
    if let Some(rest) = line.strip_prefix("summary: ") {
        return labeled_line("summary", rest, TOKYONIGHT_CYAN);
    }
    if let Some(rest) = line.strip_prefix("path: ") {
        return labeled_line("path", rest, TOKYONIGHT_BLUE);
    }
    if let Some(rest) = line.strip_prefix("error: ") {
        return labeled_line("error", rest, Color::Red);
    }

    render_diff_line(entry, line, width)
}

fn labeled_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(color)),
        Span::raw(value.to_string()),
    ])
}

fn render_diff_line(entry: &HistoryEntry, line: &str, width: usize) -> Line<'static> {
    if let Some(content) = line.strip_prefix('+').filter(|_| !line.starts_with("+++")) {
        return syntax_diff_line(
            content,
            "+",
            Color::Green,
            DIFF_ADDED_BG,
            &entry.path,
            width,
        );
    }
    if let Some(content) = line.strip_prefix('-').filter(|_| !line.starts_with("---")) {
        return syntax_diff_line(
            content,
            "-",
            Color::Red,
            DIFF_DELETED_BG,
            &entry.path,
            width,
        );
    }
    if let Some(content) = line.strip_prefix(' ') {
        return syntax_diff_line(
            content,
            " ",
            Color::DarkGray,
            Color::Reset,
            &entry.path,
            width,
        );
    }
    if line.starts_with("@@") {
        return Line::from(Span::styled(
            fit_line(line, width),
            Style::default().fg(TOKYONIGHT_CYAN),
        ));
    }
    if line.starts_with("+++") || line.starts_with("---") {
        return Line::from(Span::styled(
            fit_line(line, width),
            Style::default().fg(TOKYONIGHT_BLUE),
        ));
    }

    Line::from(Span::raw(fit_line(line, width)))
}

fn syntax_diff_line(
    content: &str,
    marker: &str,
    marker_fg: Color,
    bg: Color,
    path: &str,
    width: usize,
) -> Line<'static> {
    let base_style = Style::default().bg(bg);
    let marker_style = base_style.fg(marker_fg);
    let content_width = width.saturating_sub(2);

    let mut spans = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(" ".to_string(), base_style),
    ];
    let (mut content_spans, visual_width) =
        highlighted_spans(path, content, base_style, content_width);
    spans.append(&mut content_spans);
    let padding = content_width.saturating_sub(visual_width);
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), base_style));
    }

    Line::from(spans)
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

fn run_scripted(traces: &[LoadedTrace]) -> Result<(), String> {
    let mut state = AppState::new(traces.len());
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    for ch in input.chars() {
        match ch {
            'j' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll += 1;
                } else {
                    move_down(&mut state, traces);
                }
            }
            'k' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll = state.diff_scroll.saturating_sub(1);
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
            '1' => state.focus = Focus::Entries,
            '2' => state.focus = Focus::Traces,
            '3' => state.focus = Focus::Diff,
            'q' => break,
            _ => {}
        }
    }

    print!("{}", render_scripted(traces, &state, 120, 30));
    Ok(())
}

fn render_scripted(
    traces: &[LoadedTrace],
    state: &AppState,
    width: usize,
    height: usize,
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
            &format!("{marker} {}", entry_label(entry)),
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
    let detail = active_trace
        .entries
        .get(state.entry_index())
        .map(detail_lines)
        .unwrap_or_default();
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
            diff: Some("--- a/app.txt\n+++ b/app.txt\n-const x = 1;\n+const x = 2;\n".to_string()),
            error: None,
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
                "--- a/config.json\n+++ b/config.json\n   \"version\": 1\n+  \"version\": 2\n"
                    .to_string(),
            ),
            error: None,
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
    fn j_scrolls_diff_when_focused_on_diff() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Diff;

        handle_key(&mut state, &traces, KeyCode::Char('j'), 10, 4);
        assert_eq!(state.diff_scroll, 1);

        handle_key(&mut state, &traces, KeyCode::Char('k'), 10, 4);
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
    fn esc_quits() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        }];
        let mut state = AppState::new(traces.len());
        assert_eq!(
            handle_key(&mut state, &traces, KeyCode::Esc, 0, 0),
            AppCommand::Quit
        );
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

        let output = render_scripted(&traces, &state, 140, 30);

        assert!(output.contains("edit ok Update x constant"));
        assert!(output.contains("write ok Rewrite config"));
        assert!(output.contains("01JTESTTRA"));
        assert!(output.contains("[1] Entries 1 of 2"));
        assert!(output.contains("[2] Traces 1 of 2"));
        assert!(output.contains("tool: edit"));
    }

    #[test]
    fn scripted_render_shows_empty_message() {
        let state = AppState::new(0);
        let output = render_scripted(&[], &state, 140, 30);
        assert!(output.contains("No traces"));
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

        let output = render_scripted(&traces, &state, 140, 30);

        assert!(output.contains("> write ok Rewrite config"));
        assert!(output.contains("tool: write"));
    }
}
