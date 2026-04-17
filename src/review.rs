use std::io::{self, IsTerminal, Read};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use edit::HistoryEntry;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};

use crate::highlight::highlighted_spans;

const TOKYONIGHT_ORANGE: Color = Color::Rgb(255, 150, 108);
const TOKYONIGHT_BLUE: Color = Color::Rgb(122, 162, 247);
const TOKYONIGHT_CYAN: Color = Color::Rgb(101, 188, 255);
const LAZYGIT_ADDED_BG: Color = Color::Rgb(29, 43, 52);
const LAZYGIT_DELETED_BG: Color = Color::Rgb(48, 31, 39);

pub(crate) fn run(entries: &[HistoryEntry]) -> Result<(), String> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        run_tui(entries)
    } else {
        run_scripted(entries)
    }
}

fn run_scripted(entries: &[HistoryEntry]) -> Result<(), String> {
    let mut state = ReviewState::default();
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    for ch in input.chars() {
        match ch {
            'j' => state.move_down(entries.len()),
            'k' => state.move_up(),
            'q' => break,
            _ => {}
        }
    }

    print!("{}", render_scripted(entries, &state, 100, 30));
    Ok(())
}

fn run_tui(entries: &[HistoryEntry]) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("Failed to create review screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

    let mut app = ReviewApp::new(entries);
    loop {
        let detail_height = terminal
            .draw(|frame| draw(frame, &mut app))
            .map_err(|err| format!("Failed to render review screen: {err}"))?
            .area
            .height
            .saturating_sub(4) as usize;
        let detail_row_count = app.detail_row_count(app.state.selected);

        match event::read().map_err(|err| format!("Failed to read input event: {err}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(
                    &mut app.state,
                    key.code,
                    entries.len(),
                    detail_row_count,
                    detail_height,
                ) == ReviewCommand::Quit
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
        .map_err(|err| format!("Failed to enter review screen: {err}"))?;
        terminal
            .clear()
            .map_err(|err| format!("Failed to clear review screen: {err}"))?;
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

#[derive(Debug)]
struct ReviewApp<'a> {
    entries: &'a [HistoryEntry],
    state: ReviewState,
    cache: ReviewCache,
}

impl<'a> ReviewApp<'a> {
    fn new(entries: &'a [HistoryEntry]) -> Self {
        Self {
            entries,
            state: ReviewState::default(),
            cache: ReviewCache::new(entries.len()),
        }
    }

    fn detail_row_count(&mut self, index: usize) -> usize {
        self.cache
            .detail_lines(index, &self.entries[index])
            .iter()
            .filter(|line| !line.starts_with("edit: "))
            .count()
    }

    fn rendered_detail_lines(&mut self, index: usize, width: usize) -> &[Line<'static>] {
        self.cache
            .rendered_detail_lines(index, &self.entries[index], width)
    }
}

#[derive(Debug)]
struct ReviewCache {
    entries: Vec<EntryCache>,
    #[cfg(test)]
    model_builds: usize,
    #[cfg(test)]
    render_builds: usize,
}

impl ReviewCache {
    fn new(entry_count: usize) -> Self {
        Self {
            entries: (0..entry_count).map(|_| EntryCache::default()).collect(),
            #[cfg(test)]
            model_builds: 0,
            #[cfg(test)]
            render_builds: 0,
        }
    }

    fn detail_lines<'a>(&'a mut self, index: usize, entry: &HistoryEntry) -> &'a [String] {
        let cached = &mut self.entries[index];
        if cached.detail_lines.is_none() {
            cached.detail_lines = Some(detail_lines(entry));
            #[cfg(test)]
            {
                self.model_builds += 1;
            }
        }
        cached.detail_lines.as_deref().unwrap_or(&[])
    }

    fn rendered_detail_lines<'a>(
        &'a mut self,
        index: usize,
        entry: &HistoryEntry,
        width: usize,
    ) -> &'a [Line<'static>] {
        let cached = &mut self.entries[index];
        if cached.detail_lines.is_none() {
            cached.detail_lines = Some(detail_lines(entry));
            #[cfg(test)]
            {
                self.model_builds += 1;
            }
        }

        let needs_render = cached
            .rendered_detail
            .as_ref()
            .is_none_or(|rendered| rendered.width != width);
        if needs_render {
            let lines = cached
                .detail_lines
                .as_ref()
                .expect("detail lines should be cached before render")
                .iter()
                .filter(|line| !line.starts_with("edit: "))
                .map(|line| render_detail_line(entry, line, width))
                .collect();
            cached.rendered_detail = Some(RenderedDetail { width, lines });
            #[cfg(test)]
            {
                self.render_builds += 1;
            }
        }

        cached
            .rendered_detail
            .as_ref()
            .map(|rendered| rendered.lines.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Default)]
struct EntryCache {
    detail_lines: Option<Vec<String>>,
    rendered_detail: Option<RenderedDetail>,
}

#[derive(Debug)]
struct RenderedDetail {
    width: usize,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ReviewState {
    selected: usize,
    detail_scroll: usize,
}

impl ReviewState {
    fn move_down(&mut self, entry_count: usize) {
        if entry_count == 0 {
            return;
        }
        if self.selected + 1 < entry_count {
            self.selected += 1;
            self.detail_scroll = 0;
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.detail_scroll = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewCommand {
    Continue,
    Quit,
}

fn handle_key(
    state: &mut ReviewState,
    key: KeyCode,
    entry_count: usize,
    detail_row_count: usize,
    detail_height: usize,
) -> ReviewCommand {
    match key {
        KeyCode::Char('q') => ReviewCommand::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            state.move_down(entry_count);
            ReviewCommand::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.move_up();
            ReviewCommand::Continue
        }
        KeyCode::PageDown => {
            let max_scroll = max_detail_scroll(detail_row_count, detail_height);
            state.detail_scroll = (state.detail_scroll + detail_height.max(1)).min(max_scroll);
            ReviewCommand::Continue
        }
        KeyCode::PageUp => {
            state.detail_scroll = state.detail_scroll.saturating_sub(detail_height.max(1));
            ReviewCommand::Continue
        }
        _ => ReviewCommand::Continue,
    }
}

fn max_detail_scroll(detail_row_count: usize, detail_height: usize) -> usize {
    detail_row_count.saturating_sub(detail_height.max(1))
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &mut ReviewApp<'_>) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(layout[0]);

    let items = app
        .entries
        .iter()
        .enumerate()
        .map(|(_, entry)| ListItem::new(entry_label(entry)))
        .collect::<Vec<_>>();
    let mut list_state = ListState::default().with_selected(Some(app.state.selected));
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Entries ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(TOKYONIGHT_ORANGE)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, body[0], &mut list_state);

    let detail = Paragraph::new(
        app.rendered_detail_lines(app.state.selected, body[1].width.saturating_sub(2) as usize)
            .to_vec(),
    )
    .block(
        Block::default()
            .title(" Detail ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(TOKYONIGHT_BLUE)),
    )
    .scroll((app.state.detail_scroll as u16, 0));
    frame.render_widget(detail, body[1]);

    frame.render_widget(
        Paragraph::new("j/k move  arrows move  PgUp/PgDn scroll  q quit")
            .style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
}

fn entry_label(entry: &HistoryEntry) -> String {
    let status = if entry.ok { "ok" } else { "fail" };
    format!("{} {} {}", entry.tool, status, entry.summary)
}

fn detail_lines(entry: &HistoryEntry) -> Vec<String> {
    let mut lines = vec![
        format!("tool: {}", entry.tool),
        format!("summary: {}", entry.summary),
        format!("path: {}", entry.path),
    ];

    if entry.tool == "edit" {
        for edit in &entry.edits {
            lines.push(format!("edit: {}", edit.summary));
        }
    }

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
    if let Some(rest) = line.strip_prefix("edit: ") {
        return labeled_line("edit", rest, TOKYONIGHT_ORANGE);
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
            LAZYGIT_ADDED_BG,
            &entry.path,
            width,
        );
    }
    if let Some(content) = line.strip_prefix('-').filter(|_| !line.starts_with("---")) {
        return syntax_diff_line(
            content,
            "-",
            Color::Red,
            LAZYGIT_DELETED_BG,
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

fn render_scripted(
    entries: &[HistoryEntry],
    state: &ReviewState,
    width: usize,
    height: usize,
) -> String {
    let left_width = (width / 3).max(24).min(width.saturating_sub(20));
    let right_width = width.saturating_sub(left_width + 3);

    let left_lines = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let marker = if index == state.selected { ">" } else { " " };
            fit_line(&format!("{marker} {}", entry_label(entry)), left_width)
        })
        .collect::<Vec<_>>();

    let detail = detail_lines(&entries[state.selected])
        .into_iter()
        .filter(|line| !line.starts_with("edit: "))
        .collect::<Vec<_>>();
    let visible_detail = detail
        .iter()
        .skip(state.detail_scroll)
        .take(height.max(1))
        .map(|line| fit_line(line, right_width))
        .collect::<Vec<_>>();

    let row_count = height
        .max(left_lines.len())
        .max(visible_detail.len())
        .max(1);
    let mut output = String::new();
    for row in 0..row_count {
        let left = left_lines.get(row).map(String::as_str).unwrap_or("");
        let right = visible_detail.get(row).map(String::as_str).unwrap_or("");
        output.push_str(&format!("{left:<left_width$} | {right}\n"));
    }
    output
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

#[cfg(test)]
mod tests {
    use edit::{HistoryEntry, TextEdit};
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    use super::{
        LAZYGIT_ADDED_BG, ReviewApp, ReviewCommand, ReviewState, detail_lines, draw, handle_key,
        max_detail_scroll, render_diff_line,
    };

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

    fn rust_edit_entry() -> HistoryEntry {
        HistoryEntry {
            v: 1,
            tool: "edit".to_string(),
            trace_id: "01JTESTTRACE00000000000000".to_string(),
            timestamp: "2026-04-16T12:00:00Z".to_string(),
            cwd: "/tmp/project".to_string(),
            path: "/tmp/project/main.rs".to_string(),
            summary: "Update main".to_string(),
            ok: true,
            edits: vec![TextEdit {
                summary: "Edit main".to_string(),
                old_text: "fn main() {}".to_string(),
                new_text: "fn main() { println!(\"hi\"); }".to_string(),
            }],
            content: String::new(),
            diff: Some(
                "--- a/main.rs\n+++ b/main.rs\n-fn main() {}\n+fn main() { println!(\"hi\"); }\n"
                    .to_string(),
            ),
            error: None,
        }
    }

    fn edit_entry_with_block_suffix() -> HistoryEntry {
        HistoryEntry {
            summary: "Edit src/review.rs (3 blocks)".to_string(),
            ..edit_entry()
        }
    }

    fn write_failure() -> HistoryEntry {
        HistoryEntry {
            v: 1,
            tool: "write".to_string(),
            trace_id: "01JTESTTRACE00000000000000".to_string(),
            timestamp: "2026-04-16T12:01:00Z".to_string(),
            cwd: "/tmp/project".to_string(),
            path: "/tmp/project/config.json".to_string(),
            summary: "Rewrite config".to_string(),
            ok: false,
            edits: Vec::new(),
            content: String::new(),
            diff: None,
            error: Some("Path is not a file: /tmp/project/config.json".to_string()),
        }
    }

    #[test]
    fn renders_selected_entry_and_detail() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry(), write_failure()];
        let mut app = ReviewApp::new(&entries);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("edit ok Update x constant"));
        assert!(!text.contains("1 edit ok Update x constant"));
        assert!(text.contains("tool: edit"));
        assert!(!text.contains("edit: Edit change"));
        assert!(!text.contains("edit 1: Edit change"));
        assert!(text.contains("+ "));
        assert!(text.contains("const x = 2;"));
    }

    #[test]
    fn shows_error_for_failed_entry() {
        let lines = detail_lines(&write_failure());
        assert!(lines.contains(&"error: Path is not a file: /tmp/project/config.json".to_string()));
    }

    #[test]
    fn moving_selection_resets_detail_scroll() {
        let entries = vec![edit_entry(), write_failure()];
        let mut state = ReviewState {
            selected: 0,
            detail_scroll: 5,
        };

        let command = handle_key(
            &mut state,
            crossterm::event::KeyCode::Char('j'),
            entries.len(),
            detail_lines(&entries[0]).len(),
            5,
        );

        assert_eq!(command, ReviewCommand::Continue);
        assert_eq!(state.selected, 1);
        assert_eq!(state.detail_scroll, 0);
    }

    #[test]
    fn detail_scroll_clamps_to_bounds() {
        let entries = vec![edit_entry()];
        let mut state = ReviewState::default();
        let detail_height = 2;
        let row_count = detail_lines(&entries[0]).len();
        let max_scroll = max_detail_scroll(row_count, detail_height);

        for _ in 0..10 {
            let _ = handle_key(
                &mut state,
                crossterm::event::KeyCode::PageDown,
                entries.len(),
                row_count,
                detail_height,
            );
        }

        assert_eq!(state.detail_scroll, max_scroll);
    }

    #[test]
    fn scroll_changes_visible_detail_lines() {
        let backend = TestBackend::new(140, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);
        app.state.detail_scroll = 4;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!text.contains("tool: edit"));
        assert!(text.contains("--- a/app.txt"));
        assert!(text.contains("+ "));
        assert!(text.contains("const x = 2;"));
    }

    #[test]
    fn renders_tokyonight_pane_borders() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry(), write_failure()];
        let mut app = ReviewApp::new(&entries);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        assert!(buffer.content().iter().any(|cell| {
            cell.fg == Color::Rgb(255, 150, 108)
                && matches!(cell.symbol(), "╭" | "╮" | "╰" | "╯" | "│" | "─")
        }));
        assert!(buffer.content().iter().any(|cell| {
            cell.fg == Color::Rgb(122, 162, 247)
                && matches!(cell.symbol(), "╭" | "╮" | "╰" | "╯" | "│" | "─")
        }));
    }

    #[test]
    fn renders_raw_summary_in_tui() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry_with_block_suffix()];
        let mut app = ReviewApp::new(&entries);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("edit ok Edit src/review.rs (3 blocks)"));
        assert!(text.contains("summary: Edit src/review.rs (3 blocks)"));
    }

    #[test]
    fn diff_rows_use_syntax_colors_and_keep_added_background() {
        let line = render_diff_line(&rust_edit_entry(), "+fn main() { println!(\"hi\"); }", 60);

        assert!(
            line.spans
                .iter()
                .any(|span| span.style.fg != Some(Color::Reset))
        );
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.bg == Some(LAZYGIT_ADDED_BG))
        );
    }

    #[test]
    fn caches_detail_model_per_entry() {
        let entries = vec![edit_entry(), write_failure()];
        let mut app = ReviewApp::new(&entries);
        let expected = detail_lines(&entries[0])
            .iter()
            .filter(|line| !line.starts_with("edit: "))
            .count();

        assert_eq!(app.cache.model_builds, 0);
        assert_eq!(app.detail_row_count(0), expected);
        assert_eq!(app.cache.model_builds, 1);

        assert_eq!(app.detail_row_count(0), expected);
        assert_eq!(app.cache.model_builds, 1);
    }

    fn edit_entry_with_many_edits() -> HistoryEntry {
        let edits = (0..5)
            .map(|i| TextEdit {
                summary: format!("Edit {i}"),
                old_text: format!("old_{i}"),
                new_text: format!("new_{i}"),
            })
            .collect();
        HistoryEntry {
            edits,
            ..edit_entry()
        }
    }

    #[test]
    fn detail_row_count_matches_rendered_pane() {
        let entries = vec![edit_entry_with_many_edits()];
        let mut app = ReviewApp::new(&entries);

        let row_count = app.detail_row_count(0);
        let rendered_len = app.rendered_detail_lines(0, 60).len();

        assert_eq!(row_count, rendered_len);
        assert!(row_count < detail_lines(&entries[0]).len());
    }

    #[test]
    fn scripted_render_hides_edit_rows() {
        let entries = vec![edit_entry_with_many_edits()];
        let state = ReviewState::default();

        let output = super::render_scripted(&entries, &state, 140, 40);

        assert!(output.contains("tool: edit"));
        assert!(output.contains("summary: Update x constant"));
        assert!(!output.contains("edit: Edit 0"));
        assert!(!output.contains("edit: Edit 4"));
    }

    #[test]
    fn does_not_render_top_label() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!text.contains("Trace review"));
    }

    #[test]
    fn reuses_rendered_detail_for_same_entry_and_width() {
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);

        let first = app.rendered_detail_lines(0, 60).to_vec();
        assert_eq!(app.cache.render_builds, 1);

        let second = app.rendered_detail_lines(0, 60).to_vec();
        assert_eq!(app.cache.render_builds, 1);
        assert_eq!(second, first);
    }

    #[test]
    fn draw_reuses_cached_detail_render() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.cache.render_builds, 1);

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.cache.render_builds, 1);
    }

    #[test]
    fn page_down_uses_cached_row_count() {
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);
        let row_count = app.detail_row_count(0);
        let mut state = ReviewState::default();

        let mut command = ReviewCommand::Continue;
        for _ in 0..10 {
            command = handle_key(
                &mut state,
                crossterm::event::KeyCode::PageDown,
                entries.len(),
                row_count,
                2,
            );
        }

        assert_eq!(command, ReviewCommand::Continue);
        assert_eq!(state.detail_scroll, row_count.saturating_sub(2));
        assert_eq!(app.cache.model_builds, 1);
    }

    #[test]
    fn width_change_rebuilds_rendered_detail_once() {
        let entries = vec![edit_entry()];
        let mut app = ReviewApp::new(&entries);

        let first = app.rendered_detail_lines(0, 60).to_vec();
        assert_eq!(app.cache.render_builds, 1);

        let second = app.rendered_detail_lines(0, 80).to_vec();
        assert_eq!(app.cache.render_builds, 2);
        assert_ne!(second, first);

        let third = app.rendered_detail_lines(0, 80).to_vec();
        assert_eq!(app.cache.render_builds, 2);
        assert_eq!(third, second);
    }

    #[test]
    fn builds_each_entry_cache_lazily() {
        let entries = vec![edit_entry(), write_failure()];
        let mut app = ReviewApp::new(&entries);

        let _ = app.rendered_detail_lines(0, 60);
        assert_eq!(app.cache.model_builds, 1);
        assert_eq!(app.cache.render_builds, 1);

        let _ = app.rendered_detail_lines(1, 60);
        assert_eq!(app.cache.model_builds, 2);
        assert_eq!(app.cache.render_builds, 2);
    }
}
