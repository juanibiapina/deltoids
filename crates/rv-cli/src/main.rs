//! rv: render a git diff with the deltoids look in a scrollable TUI.
//!
//! Mirrors the input pipeline of the `deltoids` CLI exactly: read a
//! unified diff from stdin, parse it, resolve before/after blob content
//! against the local repo, and compute per-file [`Diff`]s. Instead of
//! emitting ANSI text for `less`, render hunks as ratatui
//! [`Line<'static>`] values and scroll them in an alternate screen.
//!
//! Usage:
//!
//! ```sh
//! git diff | rv
//! ```
//!
//! Layout:
//!
//! - Left sidebar — file tree with status badges, nerd icons, and
//!   per-file line-delta counts (lazygit-inspired). Selecting a file
//!   scrolls the diff pane to that file's header.
//! - Right pane — the deltoids diff renderer, scrollable.
//!
//! Keys:
//!
//! - `Tab` / `1` / `2` — focus sidebar / diff.
//! - `j`/`k` — move selection (sidebar) or scroll one line (diff).
//! - `Shift+J`/`Shift+K` — scroll diff three lines, regardless of focus.
//! - `PgDn`/`PgUp` / `Space` — page (current focus).
//! - `g`/`G` / `Home`/`End` — jump to top/bottom (current focus).
//! - `q`/`Esc` — quit.
//!
//! Set `RV_NO_ICONS=1` to disable nerd-font glyphs in the sidebar.

use std::io::{self, IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::symbols::scrollbar as scrollbar_symbols;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render_tui::{self, rgb_to_color};
use deltoids::{Diff, LineKind, Theme, content, git};

mod sidebar;

use sidebar::{Sidebar, SidebarFile};

const SCROLL_STEP_SMALL: usize = 1;
const SCROLL_STEP_LARGE: usize = 3;

/// Default sidebar width in columns (clamped against the terminal width
/// at draw time). Picked to fit a typical "crates/deltoids/src/" + file
/// row without truncation.
const DEFAULT_SIDEBAR_WIDTH: u16 = 36;
/// Below this terminal width the sidebar is hidden entirely.
const MIN_TERMINAL_WIDTH_FOR_SIDEBAR: u16 = 80;

fn main() {
    if let Err(err) = run() {
        eprintln!("rv: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("failed to read stdin: {err}"))?;

    if input.is_empty() {
        return Ok(());
    }

    if !io::stdout().is_terminal() {
        return Err(
            "stdout must be a terminal (rv is interactive); pipe diffs into rv, not out of it"
                .to_string(),
        );
    }

    let theme = Theme::load();
    let parsed = GitDiff::parse(&input);
    let repo = git::Repo::discover();
    let resolved = resolve(&parsed, repo.as_ref())?;
    let diffs = precompute_diffs(&resolved);

    run_tui(&resolved, &diffs, &theme)
}

/// One file's resolved content, ready for rendering.
#[cfg_attr(test, derive(Debug))]
struct ResolvedFile<'a> {
    file: &'a FileDiff,
    before: String,
    after: String,
}

/// Resolve content for every file. Returns the resolved files on success,
/// or a string describing the first missing blob on failure.
fn resolve<'a>(
    parsed: &'a GitDiff,
    repo: Option<&git::Repo>,
) -> Result<Vec<ResolvedFile<'a>>, String> {
    let mut files = Vec::with_capacity(parsed.files.len());

    for file in &parsed.files {
        let resolved = content::retrieve(file, repo);
        let before = match resolved.before {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(file)));
            }
        };
        let after = match resolved.after {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(file)));
            }
        };
        files.push(ResolvedFile {
            file,
            before,
            after,
        });
    }

    Ok(files)
}

fn missing_blob_message(hash: &str, path: &str) -> String {
    format!(
        "missing index blob {hash} for {path} \u{2014} not found in local repository\n\
         hint: fetch the source ref (e.g. `git fetch <remote> <ref>`) and try again"
    )
}

use sidebar::display_path;

/// Compute one [`Diff`] per resolved file. Done once at startup so the
/// diff pane and the sidebar share the same line-count totals.
fn precompute_diffs(files: &[ResolvedFile<'_>]) -> Vec<Diff> {
    files
        .iter()
        .map(|f| Diff::compute(&f.before, &f.after, display_path(f.file)))
        .collect()
}

/// Sum added/deleted line counts across all hunks of one diff.
fn count_deltas(diff: &Diff) -> (usize, usize) {
    let mut added = 0;
    let mut deleted = 0;
    for hunk in diff.hunks() {
        for line in &hunk.lines {
            match line.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => deleted += 1,
                LineKind::Context => {}
            }
        }
    }
    (added, deleted)
}

// ---------------------------------------------------------------------------
// View construction
// ---------------------------------------------------------------------------

/// Result of laying out all files into a single scrollable line stream.
struct DiffView {
    lines: Vec<Line<'static>>,
    /// `file_offsets[i]` is the row in `lines` where file `i`'s header
    /// starts. Used by the sidebar to scroll the diff pane in sync with
    /// file selection.
    file_offsets: Vec<usize>,
}

/// Build the diff pane as a flat list of ratatui lines. Same layout as
/// before; renders files in `display_order` (sidebar tree order) so the
/// diff pane's vertical layout matches the sidebar exactly. The
/// returned `file_offsets` is keyed by *input* index — the caller looks
/// up `file_offsets[input_index]` to find where that file's header
/// starts in the rendered output.
fn build_view(
    files: &[ResolvedFile<'_>],
    diffs: &[Diff],
    display_order: &[usize],
    width: usize,
    theme: &Theme,
) -> DiffView {
    let mut lines = Vec::new();
    let mut file_offsets = vec![0usize; files.len()];

    for (display_idx, &input_idx) in display_order.iter().enumerate() {
        if display_idx > 0 {
            lines.push(Line::from(""));
        }
        file_offsets[input_idx] = lines.len();

        let resolved = &files[input_idx];
        let path = display_path(resolved.file);
        lines.extend(render_tui::render_file_header(path, width, theme));

        if let Some(old_path) = &resolved.file.rename_from {
            lines.push(render_tui::render_rename_header(
                old_path,
                &resolved.file.new_path,
                theme,
            ));
        }

        let diff = &diffs[input_idx];
        for hunk in diff.hunks() {
            lines.push(Line::from(""));
            lines.extend(render_tui::render_hunk(hunk, diff.language(), width, theme));
        }
    }

    DiffView {
        lines,
        file_offsets,
    }
}

// ---------------------------------------------------------------------------
// Scroll state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Diff,
}

struct ViewState {
    /// Cached diff lines, valid for `cached_width`.
    diff_lines: Vec<Line<'static>>,
    /// Per-file row offsets into `diff_lines`. Indexed by *input* index;
    /// the value is the line in `diff_lines` where that file starts.
    file_offsets: Vec<usize>,
    /// File indices in sidebar (display) order. Cached so resize
    /// rebuilds reuse the same order.
    display_order: Vec<usize>,
    /// The width `diff_lines` was built for; rebuild when the diff pane
    /// resizes.
    cached_width: usize,
    /// Vertical scroll offset (in lines) for the diff pane.
    diff_scroll: usize,
    /// Sidebar state: rows, selection, scroll. Built once at startup.
    sidebar: Sidebar,
    /// Currently-focused pane. Determines where j/k/g/G/PgUp/PgDn go.
    focus: Focus,
}

impl ViewState {
    fn new(view: DiffView, sidebar: Sidebar, display_order: Vec<usize>, width: usize) -> Self {
        Self {
            diff_lines: view.lines,
            file_offsets: view.file_offsets,
            display_order,
            cached_width: width,
            diff_scroll: 0,
            sidebar,
            focus: Focus::Sidebar,
        }
    }

    /// Maximum scroll offset given the visible viewport height.
    fn max_diff_scroll(&self, viewport: usize) -> usize {
        self.diff_lines.len().saturating_sub(viewport.max(1))
    }

    fn scroll_diff_by(&mut self, delta: isize, viewport: usize) {
        let max = self.max_diff_scroll(viewport) as isize;
        let target = (self.diff_scroll as isize + delta).clamp(0, max);
        self.diff_scroll = target as usize;
    }

    fn scroll_diff_to_top(&mut self) {
        self.diff_scroll = 0;
    }

    fn scroll_diff_to_bottom(&mut self, viewport: usize) {
        self.diff_scroll = self.max_diff_scroll(viewport);
    }

    /// Sync the diff pane's scroll to the sidebar's selected file. No-op
    /// when no file is selected.
    fn snap_diff_to_selected_file(&mut self, viewport: usize) {
        let Some(file_idx) = self.sidebar.selected_file_index() else {
            return;
        };
        let Some(&offset) = self.file_offsets.get(file_idx) else {
            return;
        };
        let max = self.max_diff_scroll(viewport);
        self.diff_scroll = offset.min(max);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    Continue,
    Quit,
}

fn handle_key(
    state: &mut ViewState,
    key: KeyCode,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
        KeyCode::Tab => {
            state.focus = match state.focus {
                Focus::Sidebar => Focus::Diff,
                Focus::Diff => Focus::Sidebar,
            };
            AppCommand::Continue
        }
        KeyCode::BackTab => {
            // Two panes: BackTab is the same toggle.
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
        // Shift+J/K always scroll the diff regardless of focus, matching
        // edit-tui — useful when navigating files but wanting to peek at
        // a long diff.
        KeyCode::Char('J') => {
            state.scroll_diff_by(SCROLL_STEP_LARGE as isize, diff_viewport);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state.scroll_diff_by(-(SCROLL_STEP_LARGE as isize), diff_viewport);
            AppCommand::Continue
        }
        KeyCode::Char('j') | KeyCode::Down => match state.focus {
            Focus::Sidebar => {
                state.sidebar.move_down(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(SCROLL_STEP_SMALL as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match state.focus {
            Focus::Sidebar => {
                state.sidebar.move_up(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(-(SCROLL_STEP_SMALL as isize), diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::PageDown | KeyCode::Char(' ') => match state.focus {
            Focus::Sidebar => {
                state.sidebar.page_down(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(diff_viewport.max(1) as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::PageUp => match state.focus {
            Focus::Sidebar => {
                state.sidebar.page_up(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(-(diff_viewport.max(1) as isize), diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::Char('g') | KeyCode::Home => match state.focus {
            Focus::Sidebar => {
                state.sidebar.top(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_to_top();
                AppCommand::Continue
            }
        },
        KeyCode::Char('G') | KeyCode::End => match state.focus {
            Focus::Sidebar => {
                state.sidebar.bottom(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_to_bottom(diff_viewport);
                AppCommand::Continue
            }
        },
        _ => AppCommand::Continue,
    }
}

// ---------------------------------------------------------------------------
// TUI loop
// ---------------------------------------------------------------------------

/// Compute the sidebar's column width given the terminal width. Returns
/// 0 when the terminal is too narrow to comfortably show the sidebar.
fn sidebar_width(terminal_width: u16) -> u16 {
    if terminal_width < MIN_TERMINAL_WIDTH_FOR_SIDEBAR {
        return 0;
    }
    DEFAULT_SIDEBAR_WIDTH.min(terminal_width / 3)
}

fn run_tui(files: &[ResolvedFile<'_>], diffs: &[Diff], theme: &Theme) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

    // Build sidebar from the resolved files plus per-file delta counts.
    let sidebar_files: Vec<SidebarFile<'_>> = files
        .iter()
        .zip(diffs.iter())
        .map(|(f, d)| {
            let (added, deleted) = count_deltas(d);
            SidebarFile {
                file: f.file,
                added,
                deleted,
            }
        })
        .collect();
    let sidebar = Sidebar::build(&sidebar_files, theme);

    // Build the diff view for the initial diff-pane width, then rebuild
    // on resize.
    let initial_total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    let initial_diff_width = diff_pane_width(initial_total_width);

    let display_order = sidebar.display_order();
    let view = build_view(files, diffs, &display_order, initial_diff_width, theme);
    let mut state = ViewState::new(view, sidebar, display_order, initial_diff_width);

    // Snap diff to the sidebar's initial selection. The viewport isn't
    // known until the first draw, so approximate it from terminal
    // height; the next iteration's resize check fixes any mismatch.
    let initial_diff_viewport = terminal
        .size()
        .map(|s| s.height.saturating_sub(1) as usize)
        .unwrap_or(40);
    state.snap_diff_to_selected_file(initial_diff_viewport);

    loop {
        // Draw and capture viewport metrics for the current frame.
        let metrics = terminal
            .draw(|frame| draw(frame, &mut state, theme))
            .map_err(|err| format!("failed to render screen: {err}"))?;
        let total_width = metrics.area.width;
        let total_height = metrics.area.height;
        let diff_width = diff_pane_width(total_width);
        let diff_viewport = total_height.saturating_sub(1) as usize; // -1 for help bar
        let sidebar_viewport = total_height.saturating_sub(1) as usize;

        // Rebuild the diff line cache if the diff pane changed width.
        if diff_width != state.cached_width && diff_width > 0 {
            let view = build_view(files, diffs, &state.display_order, diff_width, theme);
            state.diff_lines = view.lines;
            state.file_offsets = view.file_offsets;
            state.cached_width = diff_width;
            // Clamp scroll to the new content length.
            let max = state.max_diff_scroll(diff_viewport);
            if state.diff_scroll > max {
                state.diff_scroll = max;
            }
        }

        let cmd = read_event(diff_viewport, sidebar_viewport, &mut state)?;
        if cmd == AppCommand::Quit {
            break;
        }
    }

    Ok(())
}

/// Width budget for the diff pane (terminal minus sidebar minus
/// separator column).
fn diff_pane_width(terminal_width: u16) -> usize {
    let sw = sidebar_width(terminal_width);
    let separator = if sw > 0 { 1 } else { 0 };
    terminal_width.saturating_sub(sw + separator) as usize
}

fn read_event(
    diff_viewport: usize,
    sidebar_viewport: usize,
    state: &mut ViewState,
) -> Result<AppCommand, String> {
    use std::time::Duration;

    if !event::poll(Duration::from_millis(250))
        .map_err(|err| format!("failed to poll input event: {err}"))?
    {
        return Ok(AppCommand::Continue);
    }
    match event::read().map_err(|err| format!("failed to read input event: {err}"))? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            Ok(handle_key(state, key.code, diff_viewport, sidebar_viewport))
        }
        Event::Resize(_, _) => Ok(AppCommand::Continue),
        _ => Ok(AppCommand::Continue),
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &mut ViewState, theme: &Theme) {
    let area = frame.area();

    // Vertical: body | help bar.
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = root[0];
    let help_area = root[1];

    let sw = sidebar_width(body.width);
    if sw > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(sw),
                Constraint::Length(1),
                Constraint::Min(10),
            ])
            .split(body);
        draw_sidebar(frame, cols[0], state, theme);
        draw_separator(frame, cols[1], state, theme);
        draw_diff(frame, cols[2], state, theme);
    } else {
        draw_diff(frame, body, state, theme);
    }

    draw_help(frame, help_area, state, theme);
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let viewport = area.height as usize;
    let scroll = state.sidebar.scroll();
    let total = state.sidebar.row_count();
    let start = scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let visible: Vec<Line<'static>> = state.sidebar.rows()[start..end].to_vec();

    // No paragraph-level fg tint: the sidebar's spans set their own
    // colours. Focus is signalled by the separator bar's colour and by
    // the help-bar prefix.
    frame.render_widget(Paragraph::new(visible), area);

    if total > viewport.max(1) {
        let max_scroll = total.saturating_sub(viewport);
        let position = state.sidebar.selected().min(max_scroll);
        let mut scrollbar_state = ScrollbarState::new(max_scroll.saturating_add(1))
            .position(position)
            .viewport_content_length(viewport);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(scrollbar_symbols::VERTICAL)
            .thumb_symbol("\u{2590}")
            .track_style(Style::default().fg(rgb_to_color(theme.border)))
            .thumb_style(Style::default().fg(rgb_to_color(theme.border)))
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn draw_separator(frame: &mut ratatui::Frame<'_>, area: Rect, state: &ViewState, theme: &Theme) {
    // Separator colour signals focus: bright orange when the sidebar
    // is active (since it sits to its left), muted blue otherwise.
    let color = match state.focus {
        Focus::Sidebar => rgb_to_color(theme.border_active),
        Focus::Diff => rgb_to_color(theme.border),
    };
    let style = Style::default().fg(color);
    let line = "\u{2502}";
    let lines: Vec<Line<'static>> = (0..area.height)
        .map(|_| Line::styled(line, style))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_diff(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let viewport = area.height as usize;
    let total = state.diff_lines.len();
    let start = state.diff_scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let visible: Vec<Line<'static>> = state.diff_lines[start..end].to_vec();

    frame.render_widget(Paragraph::new(visible), area);

    // Vertical scrollbar pinned to the right edge.
    if total > viewport.max(1) {
        let max_scroll = total.saturating_sub(viewport);
        let mut scrollbar_state = ScrollbarState::new(max_scroll.saturating_add(1))
            .position(state.diff_scroll.min(max_scroll))
            .viewport_content_length(viewport);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(scrollbar_symbols::VERTICAL)
            .thumb_symbol("\u{2590}")
            .track_style(Style::default().fg(rgb_to_color(theme.border)))
            .thumb_style(Style::default().fg(rgb_to_color(theme.border)))
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 0,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn draw_help(frame: &mut ratatui::Frame<'_>, area: Rect, state: &ViewState, theme: &Theme) {
    let counter = match state.focus {
        Focus::Sidebar => sidebar_counter(state),
        Focus::Diff => diff_counter(state, area),
    };
    let pane = match state.focus {
        Focus::Sidebar => "sidebar",
        Focus::Diff => "diff",
    };
    let prefix = if counter.is_empty() {
        format!("[{pane}]")
    } else {
        format!("[{pane} {counter}]")
    };
    let text =
        format!("{prefix}  Tab/1/2 focus  j/k move  Shift+J/K scroll diff  g/G top/bottom  q quit");
    let p = Paragraph::new(text).style(Style::default().fg(rgb_to_color(theme.muted)));
    frame.render_widget(p, area);
}

/// Position of the selected file among all files (1-based), the total
/// file count, and the aggregate `+N -N` line counts.
fn sidebar_counter(state: &ViewState) -> String {
    let total = state.display_order.len();
    if total == 0 {
        return String::new();
    }
    let selected_input = match state.sidebar.selected_file_index() {
        Some(i) => i,
        None => return String::new(),
    };
    let pos = state
        .display_order
        .iter()
        .position(|&i| i == selected_input)
        .map(|p| p + 1)
        .unwrap_or(0);
    let totals = state.sidebar.totals();
    let mut s = format!("file {pos}/{total}");
    if totals.added > 0 || totals.deleted > 0 {
        s.push_str(" — ");
        if totals.added > 0 {
            s.push_str(&format!("+{}", totals.added));
            if totals.deleted > 0 {
                s.push(' ');
            }
        }
        if totals.deleted > 0 {
            s.push_str(&format!("-{}", totals.deleted));
        }
    }
    s
}

/// Diff scroll position summarised as `line X/Y`.
fn diff_counter(state: &ViewState, _area: Rect) -> String {
    let total = state.diff_lines.len();
    if total == 0 {
        return String::new();
    }
    format!("line {}/{}", state.diff_scroll + 1, total)
}

struct TerminalSession;

impl TerminalSession {
    fn enter<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )
        .map_err(|err| format!("failed to enter screen: {err}"))?;
        terminal
            .clear()
            .map_err(|err| format!("failed to clear screen: {err}"))?;
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
        let _ = io::stdout().flush();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    /// Build a `FileDiff` with the given path. The `hunks` field is left
    /// empty: `build_view` runs `Diff::compute` against the supplied
    /// before/after text, so the parsed hunks aren't read.
    fn file_diff(path: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: path.to_string(),
            new_path: path.to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: Vec::new(),
        }
    }

    /// Concatenate the visible text of a `Line<'static>`.
    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn make_state(files: &[ResolvedFile<'_>]) -> ViewState {
        let diffs = precompute_diffs(files);
        let sidebar_files: Vec<SidebarFile<'_>> = files
            .iter()
            .zip(diffs.iter())
            .map(|(f, d)| {
                let (added, deleted) = count_deltas(d);
                SidebarFile {
                    file: f.file,
                    added,
                    deleted,
                }
            })
            .collect();
        let sidebar = Sidebar::build_with_icons(&sidebar_files, &theme(), sidebar::IconMode::Off);
        let display_order = sidebar.display_order();
        let view = build_view(files, &diffs, &display_order, 80, &theme());
        ViewState::new(view, sidebar, display_order, 80)
    }

    #[test]
    fn build_view_emits_file_header_and_hunk_for_one_file() {
        let f = file_diff("foo.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0], 80, &theme());
        let texts: Vec<String> = view.lines.iter().map(line_text).collect();

        assert!(
            texts.iter().any(|t| t == "foo.txt"),
            "expected file header, got: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("hello")),
            "expected removed line in: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("world")),
            "expected added line in: {texts:#?}"
        );
    }

    #[test]
    fn build_view_records_one_offset_per_file() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0, 1], 80, &theme());
        assert_eq!(view.file_offsets.len(), 2);
        // First offset is 0 (no leading blank).
        assert_eq!(view.file_offsets[0], 0);
        // Second offset points at b's header line.
        let second = view.file_offsets[1];
        let header_text = line_text(&view.lines[second]);
        assert_eq!(header_text, "b.txt");
    }

    #[test]
    fn build_view_renders_in_display_order() {
        // Files supplied in input order [a, b], display order [b, a].
        // Output's first file header must be b's; offsets keyed by
        // input index.
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[1, 0], 80, &theme());
        assert_eq!(line_text(&view.lines[0]), "b.txt");
        assert_eq!(view.file_offsets[1], 0);
        assert!(view.file_offsets[0] > 0);
    }

    #[test]
    fn build_view_includes_rename_header_when_renamed() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0], 80, &theme());
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("renamed:")
                && combined.contains("old.txt")
                && combined.contains("new.txt"),
            "missing rename header in: {combined}"
        );
    }

    #[test]
    fn count_deltas_counts_added_and_removed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "old1\nold2\nshared\n".to_string(),
            after: "new1\nshared\nnew2\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let (added, deleted) = count_deltas(&diffs[0]);
        assert!(added > 0, "expected adds");
        assert!(deleted > 0, "expected dels");
    }

    #[test]
    fn handle_key_q_quits() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('q'), 4, 4),
            AppCommand::Quit
        );
    }

    #[test]
    fn handle_key_tab_toggles_focus() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
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
    fn handle_key_j_in_diff_focus_scrolls_diff() {
        // Build a diff with enough lines to scroll.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        state.focus = Focus::Diff;
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
        assert_eq!(state.diff_scroll, 1);
    }

    #[test]
    fn handle_key_j_in_sidebar_focus_moves_sidebar_and_snaps_diff() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        // Initial selection is file 0, diff_scroll 0.
        assert_eq!(state.sidebar.selected_file_index(), Some(0));
        assert_eq!(state.diff_scroll, 0);

        // Use a viewport smaller than the rendered diff so snapping
        // actually moves the scroll offset (otherwise it clamps to 0).
        handle_key(&mut state, KeyCode::Char('j'), 2, 4);
        // Sidebar should now be on file 1.
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        // Diff scroll should be at file 1's offset.
        assert_eq!(state.diff_scroll, state.file_offsets[1]);
    }

    #[test]
    fn handle_key_capital_j_scrolls_diff_in_sidebar_focus() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        // Stay in Sidebar focus; Shift+J should still scroll the diff.
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Char('J'), 4, 4);
        assert_eq!(state.diff_scroll, SCROLL_STEP_LARGE);
    }

    #[test]
    fn missing_blob_propagates_error() {
        // Forge a diff whose old blob hash is non-null and unresolvable.
        let diff = "diff --git a/foo.txt b/foo.txt\n\
                    index deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..0000000000000000000000000000000000000000 100644\n\
                    --- a/foo.txt\n\
                    +++ /dev/null\n\
                    @@ -1 +0,0 @@\n\
                    -gone\n";
        let parsed = GitDiff::parse(diff);
        let Err(err) = resolve(&parsed, None) else {
            panic!("resolve should fail on missing blob");
        };
        assert!(err.contains("missing index blob"), "got: {err}");
        assert!(err.contains("foo.txt"), "got: {err}");
    }

    #[test]
    fn sidebar_width_hides_when_terminal_is_narrow() {
        assert_eq!(sidebar_width(60), 0);
    }

    #[test]
    fn sidebar_width_caps_at_third_of_terminal() {
        // Plenty wide → use the default width.
        assert_eq!(sidebar_width(200), DEFAULT_SIDEBAR_WIDTH);
        // Narrower terminal → capped at third.
        assert_eq!(sidebar_width(90), 30);
    }
}
