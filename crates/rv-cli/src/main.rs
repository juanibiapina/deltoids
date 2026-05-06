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
//! Keys: `j`/`k` scroll one line; `Shift+J`/`Shift+K` scroll three;
//! `PgDn`/`PgUp` page; `g`/`G` jump to top/bottom; `q`/`Esc` quit.

use std::io::{self, IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Margin;
use ratatui::style::Style;
use ratatui::symbols::scrollbar as scrollbar_symbols;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render_tui::{self, rgb_to_color};
use deltoids::{Diff, Theme, content, git};

const SCROLL_STEP_SMALL: usize = 1;
const SCROLL_STEP_LARGE: usize = 3;

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

    run_tui(&resolved, &theme)
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

fn display_path(file: &FileDiff) -> &str {
    if file.new_path == "/dev/null" {
        &file.old_path
    } else {
        &file.new_path
    }
}

// ---------------------------------------------------------------------------
// View construction
// ---------------------------------------------------------------------------

/// Build the full scrollable view as a flat list of ratatui lines.
///
/// Layout per file:
///
/// 1. (blank line, except before the first file)
/// 2. file header (2 lines)
/// 3. rename header (1 line, if applicable)
/// 4. for each hunk: blank line + `render_hunk` output
fn build_lines(files: &[ResolvedFile<'_>], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for (i, resolved) in files.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }

        let path = display_path(resolved.file);
        lines.extend(render_tui::render_file_header(path, width, theme));

        if let Some(old_path) = &resolved.file.rename_from {
            lines.push(render_tui::render_rename_header(
                old_path,
                &resolved.file.new_path,
                theme,
            ));
        }

        let diff = Diff::compute(&resolved.before, &resolved.after, path);
        for hunk in diff.hunks() {
            lines.push(Line::from(""));
            lines.extend(render_tui::render_hunk(hunk, diff.language(), width, theme));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Scroll state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ViewState {
    /// Cached lines, valid for `cached_width`.
    lines: Vec<Line<'static>>,
    /// The width `lines` was built for; rebuild when the terminal resizes.
    cached_width: usize,
    /// Vertical scroll offset (in lines).
    scroll: usize,
}

impl ViewState {
    fn new(lines: Vec<Line<'static>>, width: usize) -> Self {
        Self {
            lines,
            cached_width: width,
            scroll: 0,
        }
    }

    /// Maximum scroll offset given the visible viewport height.
    fn max_scroll(&self, viewport: usize) -> usize {
        self.lines.len().saturating_sub(viewport.max(1))
    }

    fn scroll_by(&mut self, delta: isize, viewport: usize) {
        let max = self.max_scroll(viewport) as isize;
        let target = (self.scroll as isize + delta).clamp(0, max);
        self.scroll = target as usize;
    }

    fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    fn scroll_to_bottom(&mut self, viewport: usize) {
        self.scroll = self.max_scroll(viewport);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    Continue,
    Quit,
}

fn handle_key(state: &mut ViewState, key: KeyCode, viewport: usize) -> AppCommand {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            state.scroll_by(SCROLL_STEP_SMALL as isize, viewport);
            AppCommand::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.scroll_by(-(SCROLL_STEP_SMALL as isize), viewport);
            AppCommand::Continue
        }
        KeyCode::Char('J') => {
            state.scroll_by(SCROLL_STEP_LARGE as isize, viewport);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state.scroll_by(-(SCROLL_STEP_LARGE as isize), viewport);
            AppCommand::Continue
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            state.scroll_by(viewport.max(1) as isize, viewport);
            AppCommand::Continue
        }
        KeyCode::PageUp => {
            state.scroll_by(-(viewport.max(1) as isize), viewport);
            AppCommand::Continue
        }
        KeyCode::Char('g') | KeyCode::Home => {
            state.scroll_to_top();
            AppCommand::Continue
        }
        KeyCode::Char('G') | KeyCode::End => {
            state.scroll_to_bottom(viewport);
            AppCommand::Continue
        }
        _ => AppCommand::Continue,
    }
}

// ---------------------------------------------------------------------------
// TUI loop
// ---------------------------------------------------------------------------

fn run_tui(files: &[ResolvedFile<'_>], theme: &Theme) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

    // Build lines for the initial width, then rebuild on resize.
    let initial_width = terminal
        .size()
        .map(|s| s.width as usize)
        .unwrap_or(120)
        .max(1);
    let mut state = ViewState::new(build_lines(files, initial_width, theme), initial_width);

    loop {
        let viewport = terminal
            .draw(|frame| draw(frame, &mut state, theme))
            .map_err(|err| format!("failed to render screen: {err}"))?
            .area
            .height
            .saturating_sub(0) as usize;

        // Rebuild the line cache if the terminal changed width since the
        // last build (resize between draws).
        let current_width = terminal.size().map(|s| s.width as usize).unwrap_or(0);
        if current_width != state.cached_width && current_width > 0 {
            state.lines = build_lines(files, current_width, theme);
            state.cached_width = current_width;
            // Clamp scroll to the new content length.
            let max = state.max_scroll(viewport);
            if state.scroll > max {
                state.scroll = max;
            }
        }

        let cmd = read_event(viewport, &mut state)?;
        if cmd == AppCommand::Quit {
            break;
        }
    }

    Ok(())
}

fn read_event(viewport: usize, state: &mut ViewState) -> Result<AppCommand, String> {
    use std::time::Duration;

    if !event::poll(Duration::from_millis(250))
        .map_err(|err| format!("failed to poll input event: {err}"))?
    {
        return Ok(AppCommand::Continue);
    }
    match event::read().map_err(|err| format!("failed to read input event: {err}"))? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            Ok(handle_key(state, key.code, viewport))
        }
        // Resize is handled by the cached-width check on the next iteration.
        Event::Resize(_, _) => Ok(AppCommand::Continue),
        _ => Ok(AppCommand::Continue),
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &mut ViewState, theme: &Theme) {
    let area = frame.area();

    // Slice the visible window and render as a Paragraph.
    let viewport = area.height as usize;
    let total = state.lines.len();
    let start = state.scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let visible: Vec<Line<'static>> = state.lines[start..end].to_vec();

    frame.render_widget(Paragraph::new(visible), area);

    // Vertical scrollbar pinned to the right edge.
    if total > viewport.max(1) {
        let max_scroll = total.saturating_sub(viewport);
        let mut scrollbar_state = ScrollbarState::new(max_scroll.saturating_add(1))
            .position(state.scroll.min(max_scroll))
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
    /// empty: `build_lines` runs `Diff::compute` against the supplied
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

    #[test]
    fn build_lines_emits_file_header_and_hunk_for_one_file() {
        let f = file_diff("foo.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let lines = build_lines(&resolved, 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();

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
    fn build_lines_separates_multiple_files_with_blank_line() {
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
        let lines = build_lines(&resolved, 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();

        let a_idx = texts.iter().position(|t| t == "a.txt").expect("a header");
        let b_idx = texts.iter().position(|t| t == "b.txt").expect("b header");
        assert!(a_idx < b_idx, "a should come before b");
        assert!(
            texts[a_idx + 1..b_idx].iter().any(|t| t.is_empty()),
            "expected a blank line between files, got: {:?}",
            &texts[a_idx + 1..b_idx]
        );
    }

    #[test]
    fn build_lines_includes_rename_header_when_renamed() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let lines = build_lines(&resolved, 80, &theme());
        let combined: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            combined.contains("renamed:")
                && combined.contains("old.txt")
                && combined.contains("new.txt"),
            "missing rename header in: {combined}"
        );
    }

    #[test]
    fn handle_key_q_quits() {
        let mut state = ViewState::new(vec![Line::from("a"); 10], 80);
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('q'), 4),
            AppCommand::Quit
        );
    }

    #[test]
    fn handle_key_j_scrolls_down_one_line() {
        let mut state = ViewState::new(vec![Line::from("a"); 10], 80);
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('j'), 4),
            AppCommand::Continue
        );
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn handle_key_k_does_not_go_below_zero() {
        let mut state = ViewState::new(vec![Line::from("a"); 10], 80);
        handle_key(&mut state, KeyCode::Char('k'), 4);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn handle_key_capital_g_jumps_to_bottom() {
        // 10 lines, viewport 4 -> max_scroll = 6.
        let mut state = ViewState::new(vec![Line::from("a"); 10], 80);
        handle_key(&mut state, KeyCode::Char('G'), 4);
        assert_eq!(state.scroll, 6);
    }

    #[test]
    fn handle_key_g_jumps_to_top() {
        let mut state = ViewState::new(vec![Line::from("a"); 10], 80);
        state.scroll = 5;
        handle_key(&mut state, KeyCode::Char('g'), 4);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn handle_key_pagedown_moves_by_viewport() {
        let mut state = ViewState::new(vec![Line::from("a"); 100], 80);
        handle_key(&mut state, KeyCode::PageDown, 20);
        assert_eq!(state.scroll, 20);
        handle_key(&mut state, KeyCode::PageDown, 20);
        assert_eq!(state.scroll, 40);
    }

    #[test]
    fn scroll_clamps_at_max() {
        // 5 lines, viewport 3 -> max_scroll = 2. j four times must clamp at 2.
        let mut state = ViewState::new(vec![Line::from("a"); 5], 80);
        for _ in 0..4 {
            handle_key(&mut state, KeyCode::Char('j'), 3);
        }
        assert_eq!(state.scroll, 2);
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
}
