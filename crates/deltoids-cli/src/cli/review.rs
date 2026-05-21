//! `deltoids review` — render a unified diff in a scrollable TUI.
//!
//! Mirrors the input pipeline of the pager exactly: read a unified diff
//! from stdin, parse it, resolve before/after blob content against the
//! local repo, and compute per-file [`Diff`]s. Instead of emitting ANSI
//! text for `less`, render hunks as ratatui [`Line<'static>`] values
//! and scroll them in an alternate screen.
//!
//! Usage:
//!
//! ```sh
//! git diff | deltoids review
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
//! - `PgDn`/`PgUp` — page (current focus).
//! - `g`/`G` / `Home`/`End` — jump to top/bottom (current focus).
//! - `q`/`Esc` — quit.
//! - `?` — toggle the help popup (lists every binding).
//!
//! Set `RV_NO_ICONS=1` to disable nerd-font glyphs in the sidebar.
//!
//! There is no always-on help bar at the bottom of the screen; the
//! diff pane's footer carries a small `? help` hint instead. The
//! popup itself is the source of truth for key bindings (see
//! [`HELP_KEYS`]).

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};

use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render_tui::{
    self, pane_block, pane_block_with_footer, pane_border_color, pane_inner_height,
    render_pane_scrollbar, rgb_to_color,
};
use deltoids::{Diff, LineKind, Theme, content, git};
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::sidebar::{Sidebar, SidebarFile, display_path};
use crate::terminal::TerminalSession;

const OVERVIEW: &str = r#"Read a unified diff on stdin and open it in a scrollable TUI.

Examples:
  git diff | deltoids review
  git show HEAD~1 | deltoids review

Set RV_NO_ICONS=1 to disable nerd-font glyphs in the sidebar.
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {}

const SCROLL_STEP_SMALL: usize = 1;
const SCROLL_STEP_LARGE: usize = 3;

/// Single source of truth for the help popup. Each entry is
/// `(keys, description)` and renders as one row of a two-column
/// table inside the popup.
const HELP_KEYS: &[(&str, &str)] = &[
    ("?", "toggle this help"),
    ("Tab / 1 / 2", "focus sidebar / diff"),
    ("j / k", "move (sidebar) or scroll one line (diff)"),
    ("Shift+J / K", "scroll diff three lines (any focus)"),
    ("PgDn / PgUp", "page in current pane"),
    ("g / G", "top / bottom of current pane"),
    ("Home / End", "top / bottom of current pane"),
    ("q / Esc", "quit (or close this popup)"),
];

/// Default sidebar width in columns, *including the two border
/// columns* (clamped against the terminal width at draw time). Picked
/// to fit a typical "crates/deltoids/src/" + file row without
/// truncation: outer 38 = inner 36.
const DEFAULT_SIDEBAR_WIDTH: u16 = 38;
/// Below this terminal width the sidebar is hidden entirely.
const MIN_TERMINAL_WIDTH_FOR_SIDEBAR: u16 = 80;

pub fn run(_args: Args) -> ExitCode {
    match run_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("rv: {err}");
            ExitCode::from(1)
        }
    }
}

fn run_inner() -> Result<(), String> {
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
    /// Whether the help popup is currently shown. While true, key
    /// dispatch is intercepted by the popup's own handler.
    help_visible: bool,
    /// Last-drawn pane rects, used for mouse hit-testing.
    sidebar_rect: Rect,
    diff_rect: Rect,
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
            help_visible: false,
            sidebar_rect: Rect::default(),
            diff_rect: Rect::default(),
        }
    }

    /// Window of `diff_lines` that should be visible right now.
    ///
    /// The diff pane is always filtered to whatever the sidebar is
    /// pointing at: a directory header narrows to that subtree's
    /// files, a file row narrows to that single file. Empty diff
    /// (no files at all) falls through to the full slice so the
    /// pane simply renders nothing.
    fn visible_diff_range(&self) -> std::ops::Range<usize> {
        let Some(display_range) = self.sidebar.selection_display_range() else {
            return 0..self.diff_lines.len();
        };
        if display_range.is_empty() || self.display_order.is_empty() {
            return 0..self.diff_lines.len();
        }
        let first_input = self.display_order[display_range.start];
        let start = self.file_offsets[first_input];
        let end = if display_range.end < self.display_order.len() {
            // Stop just before the blank separator that precedes the
            // next file. file_offsets points at the file *header*, so
            // the line immediately above is the separator.
            let next_input = self.display_order[display_range.end];
            self.file_offsets[next_input].saturating_sub(1)
        } else {
            self.diff_lines.len()
        };
        start..end
    }

    /// Maximum scroll offset (an absolute index in `diff_lines`) such
    /// that the viewport still sits inside the current visible range.
    fn max_diff_scroll(&self, viewport: usize) -> usize {
        let range = self.visible_diff_range();
        let span = range.end.saturating_sub(range.start);
        range.start + span.saturating_sub(viewport.max(1))
    }

    /// Lower bound for `diff_scroll` (start of the visible range).
    fn min_diff_scroll(&self) -> usize {
        self.visible_diff_range().start
    }

    fn scroll_diff_by(&mut self, delta: isize, viewport: usize) {
        let min = self.min_diff_scroll() as isize;
        let max = self.max_diff_scroll(viewport) as isize;
        let target = (self.diff_scroll as isize + delta).clamp(min, max.max(min));
        self.diff_scroll = target as usize;
    }

    fn scroll_diff_to_top(&mut self) {
        self.diff_scroll = self.min_diff_scroll();
    }

    fn scroll_diff_to_bottom(&mut self, viewport: usize) {
        self.diff_scroll = self.max_diff_scroll(viewport);
    }

    /// Sync the diff pane's scroll to the file the sidebar is pointing
    /// at. On a file row that's the selected file; on a directory row
    /// it's the first file inside that subtree, so the diff updates as
    /// the user traverses the tree. Scroll is also clamped to the
    /// visible range.
    fn snap_diff_to_selected_file(&mut self, viewport: usize) {
        let Some(file_idx) = self.sidebar.nearest_file_index() else {
            return;
        };
        let Some(&offset) = self.file_offsets.get(file_idx) else {
            return;
        };
        let min = self.min_diff_scroll();
        let max = self.max_diff_scroll(viewport);
        self.diff_scroll = offset.clamp(min, max.max(min));
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
    if state.help_visible {
        return handle_key_help(state, key);
    }
    match key {
        KeyCode::Char('?') => {
            state.help_visible = true;
            AppCommand::Continue
        }
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
        KeyCode::PageDown => match state.focus {
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

/// Key dispatch while the help popup is shown. `?`, `Esc`, and `q`
/// all close the popup; everything else is swallowed. `q`/`Esc` do
/// **not** quit the app while the popup is open — closing the modal
/// first matches lazygit/k9s/vim convention.
fn handle_key_help(state: &mut ViewState, key: KeyCode) -> AppCommand {
    match key {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
            state.help_visible = false;
        }
        _ => {}
    }
    AppCommand::Continue
}

fn pane_at(state: &ViewState, col: u16, row: u16) -> Option<Focus> {
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
    state: &mut ViewState,
    mouse: MouseEvent,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    if state.help_visible {
        return AppCommand::Continue;
    }

    let target = match pane_at(state, mouse.column, mouse.row) {
        Some(pane) => pane,
        None => return AppCommand::Continue,
    };

    match mouse.kind {
        MouseEventKind::ScrollDown => match target {
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
        MouseEventKind::ScrollUp => match target {
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
        MouseEventKind::Down(MouseButton::Left) => {
            state.focus = target;
            if target == Focus::Sidebar {
                let rect = state.sidebar_rect;
                let content_y = mouse.row.saturating_sub(rect.y).saturating_sub(1) as usize;
                let clicked = state.sidebar.scroll() + content_y;
                if clicked < state.sidebar.row_count() {
                    state.sidebar.set_selected(clicked, sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
            }
            AppCommand::Continue
        }
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
        // -2 for the pane's top and bottom borders. The result is
        // the number of content rows the pane shows, i.e. the scroll
        // viewport. (No bottom help bar; help is a `?`-triggered
        // popup overlay that doesn't consume layout space.)
        let pane_viewport = total_height.saturating_sub(2) as usize;
        let diff_viewport = pane_viewport;
        let sidebar_viewport = pane_viewport;

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

/// Width budget for the diff pane *content* (terminal minus the
/// sidebar pane minus this pane's own two border columns). When no
/// sidebar is shown the diff pane spans the whole terminal, still
/// minus its own two borders.
fn diff_pane_width(terminal_width: u16) -> usize {
    let sw = sidebar_width(terminal_width);
    terminal_width.saturating_sub(sw + 2) as usize
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
        Event::Mouse(mouse) => Ok(handle_mouse(state, mouse, diff_viewport, sidebar_viewport)),
        Event::Resize(_, _) => Ok(AppCommand::Continue),
        _ => Ok(AppCommand::Continue),
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &mut ViewState, theme: &Theme) {
    let area = frame.area();

    let sw = sidebar_width(area.width);
    if sw > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sw), Constraint::Min(10)])
            .split(area);
        state.sidebar_rect = cols[0];
        state.diff_rect = cols[1];
        draw_sidebar(frame, cols[0], state, theme);
        draw_diff(frame, cols[1], state, theme);
    } else {
        state.sidebar_rect = Rect::default();
        state.diff_rect = area;
        draw_diff(frame, area, state, theme);
    }

    if state.help_visible {
        draw_help_popup(frame, area, theme);
    }
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let viewport = inner.height as usize;
    let inner_width = inner.width as usize;
    let scroll = state.sidebar.scroll();
    let total = state.sidebar.row_count();
    let start = scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let mut visible: Vec<Line<'static>> = state.sidebar.rows()[start..end].to_vec();

    // Extend the selection background across the full inner pane width
    // so the highlighted row reads as a continuous bar (matching
    // lazygit and edit-tui's `List` widget). Pad against the inner
    // width so the trailing block stops just before the right border.
    if let Some(rel) = state.sidebar.selected().checked_sub(scroll)
        && rel < visible.len()
    {
        pad_selected_row(&mut visible[rel], inner_width, theme);
    }

    let color = pane_border_color(state.focus == Focus::Sidebar, theme);
    let footer = sidebar_footer(state);
    let block = pane_block_with_footer("─[1]─Files─", color, footer);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    render_pane_scrollbar(
        frame,
        area,
        total,
        state.sidebar.selected(),
        pane_inner_height(area),
        theme,
    );
}

/// Append a trailing span of `selection_bg`-styled spaces so the row's
/// highlight extends to `width`. No-op when the row is already wider
/// than the pane (ratatui clips overflow).
fn pad_selected_row(line: &mut Line<'static>, width: usize, theme: &Theme) {
    let current: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if current >= width {
        return;
    }
    let pad = width - current;
    line.spans.push(Span::styled(
        " ".repeat(pad),
        Style::default().bg(rgb_to_color(theme.selection_bg)),
    ));
}

fn draw_diff(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let viewport = inner.height as usize;
    let range = state.visible_diff_range();
    let scroll = state.diff_scroll.clamp(range.start, range.end);
    let end = scroll.saturating_add(viewport.max(1)).min(range.end);
    let visible: Vec<Line<'static>> = state.diff_lines[scroll..end].to_vec();

    let color = pane_border_color(state.focus == Focus::Diff, theme);
    let footer = diff_footer(state);
    let block = pane_block_with_footer("─[2]─Diff─", color, footer);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    // Vertical scrollbar reflects the *visible range*, not the full
    // diff: when the sidebar is on a directory the scrollbar tracks
    // progress through that subtree's files.
    let span = range.end.saturating_sub(range.start);
    let position = scroll.saturating_sub(range.start);
    render_pane_scrollbar(frame, area, span, position, pane_inner_height(area), theme);
}

/// Render the help popup as a centered, bordered overlay. Sized to
/// content (capped at 80% of the terminal in each axis), cleared
/// underneath so the panes don't bleed through. Pane chrome reuses
/// [`pane_block`] for visual consistency with the rest of the UI.
fn draw_help_popup(frame: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    let rows = build_help_lines(theme);
    let content_width = HELP_KEYS
        .iter()
        .map(|(_k, d)| help_key_column_width().saturating_add(d.width()) + 2)
        .max()
        .unwrap_or(40);
    let want_w = (content_width as u16).saturating_add(4); // 2 borders + 2 padding
    let want_h = (rows.len() as u16).saturating_add(2); // 2 borders
    let max_w = (area.width * 8 / 10).max(20);
    let max_h = (area.height * 8 / 10).max(5);
    let w = want_w.min(max_w).min(area.width);
    let h = want_h.min(max_h).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, popup);
    let block = pane_block("─Help─", pane_border_color(true, theme));
    let inner = block.inner(popup).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(rows), inner);
}

/// Width reserved for the key column in the help popup so the
/// description column lines up.
fn help_key_column_width() -> usize {
    HELP_KEYS.iter().map(|(k, _)| k.width()).max().unwrap_or(0)
}

/// Build the help popup's body as ratatui [`Line`]s. Two columns:
/// keys (theme-accent) and description (default).
fn build_help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let key_w = help_key_column_width();
    let key_style = Style::default().fg(rgb_to_color(theme.border_active));
    let desc_style = Style::default().fg(rgb_to_color(theme.muted));
    HELP_KEYS
        .iter()
        .map(|(k, d)| {
            let pad = key_w.saturating_sub(k.width());
            Line::from(vec![
                Span::styled((*k).to_string(), key_style),
                Span::raw(" ".repeat(pad)),
                Span::raw("  "),
                Span::styled((*d).to_string(), desc_style),
            ])
        })
        .collect()
}

/// Build the sidebar pane's bottom-right footer: file/dir position
/// among all files plus the aggregate `+N -N` line counts.
///
/// Returns `None` when there are no files to display.
fn sidebar_footer(state: &ViewState) -> Option<String> {
    let total = state.display_order.len();
    if total == 0 {
        return None;
    }
    let selected_input = state.sidebar.nearest_file_index()?;
    let pos = state
        .display_order
        .iter()
        .position(|&i| i == selected_input)
        .map(|p| p + 1)
        .unwrap_or(0);
    let label = if state.sidebar.selected_is_dir() {
        "dir"
    } else {
        "file"
    };
    let totals = state.sidebar.totals();
    let mut s = format!(" {label} {pos} of {total}");
    if totals.added > 0 || totals.deleted > 0 {
        s.push_str("  ");
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
    s.push(' ');
    Some(s)
}

/// Build the diff pane's bottom-right footer: `" line X of Y "` for
/// the current scroll position within the visible range, or `None`
/// when the pane is empty.
fn diff_footer(state: &ViewState) -> Option<String> {
    let range = state.visible_diff_range();
    let span = range.end.saturating_sub(range.start);
    if span == 0 {
        return None;
    }
    let pos = state
        .diff_scroll
        .saturating_sub(range.start)
        .min(span.saturating_sub(1))
        + 1;
    Some(format!(" line {pos} of {span}  \u{00b7}  ? help "))
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
        let sidebar =
            Sidebar::build_with_icons(&sidebar_files, &theme(), crate::sidebar::IconMode::Off);
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
    fn dir_filter_excludes_files_outside_subtree() {
        // Three files under three different dirs. Each file's diff has
        // a unique marker line so we can assert exactly which files are
        // visible at any time.
        let a = file_diff("alpha/a.rs");
        let b = file_diff("beta/b.rs");
        let c = file_diff("gamma/c.rs");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "old_alpha\n".to_string(),
                after: "new_alpha\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "old_beta\n".to_string(),
                after: "new_beta\n".to_string(),
            },
            ResolvedFile {
                file: &c,
                before: "old_gamma\n".to_string(),
                after: "new_gamma\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        // Walk to the `beta/` dir header. Tree order: alpha/ (dir 0),
        // alpha/a.rs (file 0), beta/ (dir 1), beta/b.rs (file 1),
        // gamma/ (dir 2), gamma/c.rs (file 2).
        // Initial selection is on file 0 (alpha/a.rs at row 1).
        // Step down to row 2 = beta/.
        state.sidebar.move_down(20);
        assert!(state.sidebar.selected_is_dir());

        let range = state.visible_diff_range();
        let visible_text: String = state.diff_lines[range.clone()]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        // Only beta/b.rs's content must be inside the range.
        assert!(
            visible_text.contains("beta/b.rs")
                && visible_text.contains("old_beta")
                && visible_text.contains("new_beta"),
            "beta content missing from filtered range: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("alpha/a.rs") && !visible_text.contains("old_alpha"),
            "alpha leaked into beta filter: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("gamma/c.rs") && !visible_text.contains("old_gamma"),
            "gamma leaked into beta filter: {visible_text:?}"
        );

        // Move to a file row — visible range narrows to that single file.
        state.sidebar.move_down(20); // file row inside beta/
        assert!(!state.sidebar.selected_is_dir());
        let file_range = state.visible_diff_range();
        let file_text: String = state.diff_lines[file_range]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            file_text.contains("beta/b.rs") && file_text.contains("new_beta"),
            "beta file content missing from filtered range: {file_text:?}"
        );
        assert!(
            !file_text.contains("alpha/a.rs") && !file_text.contains("gamma/c.rs"),
            "siblings leaked into single-file filter: {file_text:?}"
        );
    }

    #[test]
    fn visible_diff_range_narrows_to_subtree_on_dir_selection() {
        // Two files in different dirs: src/a.rs and other/b.rs. The
        // sidebar tree puts each under its own dir header. Selecting
        // src/ should restrict visible_diff_range to just src/a.rs's
        // lines; selecting other/ should restrict to other/b.rs.
        let a = file_diff("src/a.rs");
        let b = file_diff("other/b.rs");
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

        // Initial selection is on a file: visible range is exactly
        // that file (single-element subset of the full diff).
        let file_range = state.visible_diff_range();
        assert!(
            file_range.end - file_range.start < state.diff_lines.len(),
            "file selection should narrow to a single file's slice"
        );
        let file_first_line = line_text(&state.diff_lines[file_range.start]);
        assert!(
            file_first_line == "src/a.rs" || file_first_line == "other/b.rs",
            "expected a file header at start, got {file_first_line:?}"
        );

        // Move up onto the dir header above the first file (other/ is
        // first alphabetically among the directory rows).
        state.sidebar.top(20);
        assert!(state.sidebar.selected_is_dir());
        let narrowed = state.visible_diff_range();
        // The range must be strictly smaller than the full diff.
        assert!(
            narrowed.end - narrowed.start < state.diff_lines.len(),
            "expected subtree range to be narrower than full diff"
        );
        // The very first visible line should be the file header for
        // whichever file the dir contains.
        let first_line = line_text(&state.diff_lines[narrowed.start]);
        assert!(
            first_line == "src/a.rs" || first_line == "other/b.rs",
            "expected dir's file header at start, got {first_line:?}"
        );
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
    fn handle_key_question_mark_opens_help() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert!(!state.help_visible);
        handle_key(&mut state, KeyCode::Char('?'), 4, 4);
        assert!(state.help_visible);
    }

    #[test]
    fn handle_key_question_mark_toggles_help_closed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        handle_key(&mut state, KeyCode::Char('?'), 4, 4);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_esc_in_help_closes_popup_does_not_quit() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        let cmd = handle_key(&mut state, KeyCode::Esc, 4, 4);
        assert_eq!(cmd, AppCommand::Continue);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_q_in_help_closes_popup_does_not_quit() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        let cmd = handle_key(&mut state, KeyCode::Char('q'), 4, 4);
        assert_eq!(cmd, AppCommand::Continue);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_navigation_swallowed_while_help_visible() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        state.focus = Focus::Diff;
        state.help_visible = true;
        let scroll_before = state.diff_scroll;
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
        assert_eq!(state.diff_scroll, scroll_before);
        assert!(state.help_visible, "unrelated keys must not close popup");
    }

    #[test]
    fn diff_footer_includes_help_hint() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state(&resolved);
        let footer = diff_footer(&state).expect("footer present");
        assert!(
            footer.contains("? help"),
            "expected '? help' hint in footer, got {footer:?}"
        );
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

    fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn make_state_with_rects(files: &[ResolvedFile<'_>]) -> ViewState {
        let mut state = make_state(files);
        state.sidebar_rect = Rect::new(0, 0, 38, 20);
        state.diff_rect = Rect::new(38, 0, 82, 20);
        state
    }

    #[test]
    fn pane_at_returns_correct_focus_review() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state_with_rects(&resolved);

        assert_eq!(pane_at(&state, 5, 5), Some(Focus::Sidebar));
        assert_eq!(pane_at(&state, 50, 5), Some(Focus::Diff));
        assert_eq!(pane_at(&state, 200, 200), None);
    }

    #[test]
    fn scroll_down_on_sidebar_moves_selection() {
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
        let mut state = make_state_with_rects(&resolved);
        let initial = state.sidebar.selected();

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.sidebar.selected() > initial);
    }

    #[test]
    fn scroll_on_diff_scrolls_content() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Diff;
        let before = state.diff_scroll;

        let mouse = make_mouse(MouseEventKind::ScrollDown, 50, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff_scroll > before);

        let after_down = state.diff_scroll;
        let mouse = make_mouse(MouseEventKind::ScrollUp, 50, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff_scroll < after_down);
    }

    #[test]
    fn click_focuses_pane_review() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
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
    fn click_on_sidebar_selects_row() {
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
        let mut state = make_state_with_rects(&resolved);
        let row_count = state.sidebar.row_count();
        assert!(row_count >= 2, "need at least 2 rows for this test");

        // Sidebar rect starts at y=0, so row 1 = border,
        // row 2 = second content row (index 1). Click on the last row.
        let target_row = row_count - 1;
        let mouse_y = 1 + target_row as u16; // +1 for top border
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, mouse_y);
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.sidebar.selected(), target_row,);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn mouse_swallowed_while_help_visible() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Diff;
        state.help_visible = true;
        let scroll_before = state.diff_scroll;
        let focus_before = state.focus;

        let mouse = make_mouse(MouseEventKind::ScrollDown, 50, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(
            state.diff_scroll, scroll_before,
            "scroll should not change while help visible"
        );
        assert_eq!(
            state.focus, focus_before,
            "focus should not change while help visible"
        );
        assert!(state.help_visible, "help should stay visible");
    }

    #[test]
    fn click_outside_panes_is_noop_review() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Sidebar;

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 200, 200);
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.focus, Focus::Sidebar);
    }
}
