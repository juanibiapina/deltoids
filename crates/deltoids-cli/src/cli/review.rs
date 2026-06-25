//! `deltoids review` — render a unified diff in a scrollable TUI.
//!
//! Two input modes, selected by whether stdin is a pipe:
//!
//! - **Piped diff** (`git diff | deltoids review`): read the unified
//!   diff from stdin.
//! - **Bare in a repo** (`deltoids review`): discover the repository and
//!   show its local working-tree changes against `HEAD` — staged and
//!   unstaged edits to tracked files plus untracked files, the same set
//!   `git diff HEAD` reports.
//!
//! From there both modes share the pager's pipeline: parse the diff,
//! resolve before/after blob content against the local repo, and compute
//! per-file [`Diff`]s. Instead of emitting ANSI text for `less`, render
//! hunks as ratatui [`Line<'static>`] values and scroll them in an
//! alternate screen.
//!
//! In bare mode the working tree is watched: saving, adding, reverting,
//! or committing a tracked or untracked file re-runs the pipeline and
//! re-renders within ~200ms, preserving focus, the selected file (by
//! path), sidebar width, scroll, and the help popup. Reverting or
//! committing every change shows a "no local changes" empty state
//! instead of exiting; new edits repopulate it. Gitignored paths and
//! `.git/` churn never trigger a reload. Piped-diff mode stays static:
//! stdin is closed, so there is nothing to re-read.
//!
//! Known limitation (Linux): the watcher arms recursively
//! ([`RecursiveMode::Recursive`]), which on Linux allocates one inotify
//! watch per directory. The gitignore filter drops *events* from
//! ignored trees but does not stop inotify from *watching* them, so a
//! repo with a huge ignored tree (e.g. `node_modules`) can be slow to
//! arm and may hit `fs.inotify.max_user_watches`. macOS is unaffected
//! (FSEvents watches a whole tree with one handle). A future refinement
//! is to enumerate only non-ignored directories (e.g. via the `ignore`
//! crate) and add per-directory non-recursive watches.
//!
//! Usage:
//!
//! ```sh
//! deltoids review          # local working-tree changes
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
//! - `<`/`>` — narrow/widen the sidebar (any focus). The divider
//!   between the panes can also be dragged with the mouse.
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
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

use crate::events::read_event_burst;
use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar::{Sidebar, SidebarFile, display_path};
use crate::sidebar_width::{self, Preference};
use crate::terminal::TerminalSession;

const OVERVIEW: &str = r#"Open a diff in a scrollable TUI.

With no piped input, shows the current repo's local changes (working
tree and index vs HEAD, plus untracked files). With a diff piped in,
shows that diff instead.

Examples:
  deltoids review
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
    ("< / >", "narrow / widen sidebar"),
    ("q / Esc", "quit (or close this popup)"),
];

/// Idle poll timeout for the event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Debounce window for working-tree change events. A burst of file-system
/// notifications (an editor save touches several files, git churns the
/// index) collapses into one reload once this much wall-clock has passed
/// since the first event. Matches the traces TUI.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);
/// Minimum wall-clock gap between full diff-cache rebuilds. Rebuilding
/// the whole cache on every resize step stalls the event loop; with
/// held-key auto-repeat that stall lets a backlog of `<`/`>` presses
/// pile up and keep resizing the sidebar after the key is released
/// (scrolling never rebuilds, so it never coasts). Between rebuilds the
/// diff is drawn from the existing cache (ratatui clips it to the new
/// width) and snaps correct once resizing settles within this interval.
const DIFF_REBUILD_THROTTLE: Duration = Duration::from_millis(50);

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
    if !io::stdout().is_terminal() {
        return Err(
            "stdout must be a terminal (review is interactive); pipe diffs into review, not out of it"
                .to_string(),
        );
    }

    // Pick the diff source by whether stdin is piped. A piped diff is read
    // verbatim (the classic `git diff | deltoids review`); a bare
    // invocation in a terminal shows the repo's local working-tree changes.
    let stdin_is_tty = io::stdin().is_terminal();
    let (input, repo) = if stdin_is_tty {
        let repo = git::Repo::discover().ok_or_else(|| {
            "not a git repository; pipe a diff (e.g. `git diff | deltoids review`) \
             or run inside a repo"
                .to_string()
        })?;
        let input = repo.working_tree_diff()?;
        if input.trim().is_empty() {
            eprintln!("deltoids review: no local changes");
            return Ok(());
        }
        (input, Some(repo))
    } else {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        if input.is_empty() {
            return Ok(());
        }
        (input, git::Repo::discover())
    };

    let theme = Theme::load();
    let model = build_model(&input, repo.as_ref())?;

    // Bare mode re-diffs the working tree on change; a piped diff is a
    // closed stream with nothing to re-read, so it stays static even when
    // `repo` is `Some` (used only for blob resolution).
    let source = match (stdin_is_tty, repo.as_ref()) {
        (true, Some(repo)) => DiffSource::WorkingTree(repo),
        _ => DiffSource::Static,
    };

    run_tui(model, source, &theme)
}

/// Describes whether and how the diff can be refreshed mid-session.
enum DiffSource<'a> {
    /// Piped stdin: a closed stream, never refreshes.
    Static,
    /// Bare repo: re-diff the working tree when files change on disk.
    WorkingTree(&'a git::Repo),
}

/// The owned data the TUI renders: resolved files plus their diffs.
/// Rebuilt wholesale on each working-tree reload.
struct Model {
    files: Vec<ResolvedFile>,
    diffs: Vec<Diff>,
}

/// Parse `input`, resolve every file's before/after content against
/// `repo`, and compute per-file [`Diff`]s.
fn build_model(input: &str, repo: Option<&git::Repo>) -> Result<Model, String> {
    let parsed = GitDiff::parse(input);
    let files = resolve(parsed, repo)?;
    let diffs = precompute_diffs(&files);
    Ok(Model { files, diffs })
}

/// One file's resolved content, ready for rendering. Owns its
/// [`FileDiff`] so a [`Model`] is a self-contained owned value (no
/// borrow of the parsed diff), which lets the TUI replace it on reload.
#[cfg_attr(test, derive(Debug))]
struct ResolvedFile {
    file: FileDiff,
    before: String,
    after: String,
}

/// Resolve content for every file. Consumes the parsed diff (taking each
/// [`FileDiff`] by value). Returns the resolved files on success, or a
/// string describing the first missing blob on failure.
fn resolve(parsed: GitDiff, repo: Option<&git::Repo>) -> Result<Vec<ResolvedFile>, String> {
    let mut files = Vec::with_capacity(parsed.files.len());

    for file in parsed.files {
        let resolved = content::retrieve(&file, repo);
        let before = match resolved.before {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(&file)));
            }
        };
        let after = match resolved.after {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(&file)));
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
fn precompute_diffs(files: &[ResolvedFile]) -> Vec<Diff> {
    files
        .iter()
        .map(|f| Diff::compute(&f.before, &f.after, display_path(&f.file)))
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
    files: &[ResolvedFile],
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
        let path = display_path(&resolved.file);
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
            lines.extend(render_tui::render_hunk(
                hunk,
                diff.highlight(),
                width,
                theme,
            ));
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
    /// User's preferred sidebar width plus the policy to resolve it to
    /// an on-screen width each frame. Adjusted by `<`/`>` or by dragging
    /// the divider; clamped on use by [`Preference::effective`].
    sidebar_pref: Preference,
    /// True while the left button is held on the pane divider, so
    /// subsequent `Drag` events resize the sidebar.
    dragging_divider: bool,
    /// Translates fanned-out mouse-wheel events into proportional motion.
    wheel: WheelScroll<Focus>,
}

impl ViewState {
    fn new(
        view: DiffView,
        sidebar: Sidebar,
        display_order: Vec<usize>,
        width: usize,
        sidebar_pref: Preference,
    ) -> Self {
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
            sidebar_pref,
            dragging_divider: false,
            wheel: WheelScroll::new(),
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

#[allow(clippy::too_many_lines)]
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
        // Resize the sidebar regardless of focus. The stored value is
        // the raw preference; clamping happens in `Preference::effective`
        // at draw time.
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

/// The two adjacent border columns that form the visible divider
/// between the sidebar and diff panes: the sidebar's right border and
/// the diff's left border. `None` when the sidebar is hidden.
fn divider_columns(state: &ViewState) -> Option<(u16, u16)> {
    if state.sidebar_rect.width == 0 {
        return None;
    }
    let right_border = state.sidebar_rect.right().saturating_sub(1);
    Some((right_border, right_border.saturating_add(1)))
}

fn is_on_divider(state: &ViewState, col: u16) -> bool {
    matches!(divider_columns(state), Some((a, b)) if col == a || col == b)
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

    // Divider drag takes precedence over pane dispatch so a grab on the
    // border neither selects a sidebar row nor changes focus.
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
        MouseEventKind::ScrollDown => match target {
            Focus::Sidebar => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_down(sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::Content);
                state.scroll_diff_by((steps * SCROLL_STEP_SMALL) as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        MouseEventKind::ScrollUp => match target {
            Focus::Sidebar => {
                let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
                for _ in 0..steps {
                    state.sidebar.move_up(sidebar_viewport);
                    state.snap_diff_to_selected_file(diff_viewport);
                }
                AppCommand::Continue
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Up, ScrollKind::Content);
                state.scroll_diff_by(-((steps * SCROLL_STEP_SMALL) as isize), diff_viewport);
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

/// Install a recursive filesystem watcher for a refreshable source.
///
/// The callback only forwards each event's paths over the returned
/// channel: git2's `Repository` is `!Send` and can't move into the (Send)
/// closure, so gitignore filtering stays on the main thread. For a static
/// source (or a bare repo with no workdir) no watcher is created and the
/// receiver never fires (the sender is dropped here).
#[allow(clippy::type_complexity)]
fn spawn_watcher(
    source: &DiffSource<'_>,
) -> Result<
    (
        Option<notify::RecommendedWatcher>,
        mpsc::Receiver<Vec<PathBuf>>,
    ),
    String,
> {
    let (notify_tx, notify_rx) = mpsc::channel::<Vec<PathBuf>>();
    let watcher = match source {
        DiffSource::WorkingTree(repo) => match repo.workdir() {
            Some(workdir) => {
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if let Ok(event) = res {
                            let _ = notify_tx.send(event.paths);
                        }
                    })
                    .map_err(|err| format!("failed to create filesystem watcher: {err}"))?;
                watcher
                    .watch(workdir, RecursiveMode::Recursive)
                    .map_err(|err| format!("failed to watch {}: {err}", workdir.display()))?;
                Some(watcher)
            }
            None => None,
        },
        DiffSource::Static => None,
    };
    Ok((watcher, notify_rx))
}

#[allow(clippy::too_many_lines)]
fn run_tui(mut model: Model, source: DiffSource<'_>, theme: &Theme) -> Result<(), String> {
    let _session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;

    // Watch the working tree when the diff is refreshable. The receiver
    // never fires for a static (piped) source.
    let (_watcher, notify_rx) = spawn_watcher(&source)?;

    // Build the diff view for the initial diff-pane width, then rebuild
    // on resize.
    let initial_total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    let sidebar_pref = Preference::seeded(initial_total_width);
    let initial_sidebar_width = sidebar_pref.effective(initial_total_width);
    let initial_diff_width =
        sidebar_width::diff_pane_width(initial_sidebar_width, initial_total_width);

    let sidebar = build_sidebar(&model, theme);
    let display_order = sidebar.display_order();
    let view = build_view(
        &model.files,
        &model.diffs,
        &display_order,
        initial_diff_width,
        theme,
    );
    let mut state = ViewState::new(
        view,
        sidebar,
        display_order,
        initial_diff_width,
        sidebar_pref,
    );

    // Snap diff to the sidebar's initial selection. The viewport isn't
    // known until the first draw, so approximate it from terminal
    // height; the next iteration's resize check fixes any mismatch.
    let initial_diff_viewport = terminal
        .size()
        .map(|s| s.height.saturating_sub(1) as usize)
        .unwrap_or(40);
    state.snap_diff_to_selected_file(initial_diff_viewport);

    let mut last_rebuild = Instant::now();
    let mut dirty_since: Option<Instant> = None;
    loop {
        // Draw and capture viewport metrics for the current frame.
        let metrics = terminal
            .draw(|frame| draw(frame, &mut state, theme))
            .map_err(|err| format!("failed to render screen: {err}"))?;
        let total_width = metrics.area.width;
        let total_height = metrics.area.height;
        let sidebar_w = state.sidebar_pref.effective(total_width);
        let diff_width = sidebar_width::diff_pane_width(sidebar_w, total_width);
        // -2 for the pane's top and bottom borders. The result is
        // the number of content rows the pane shows, i.e. the scroll
        // viewport. (No bottom help bar; help is a `?`-triggered
        // popup overlay that doesn't consume layout space.)
        let pane_viewport = total_height.saturating_sub(2) as usize;
        let diff_viewport = pane_viewport;
        let sidebar_viewport = pane_viewport;

        // Rebuild the diff line cache if the diff pane changed width, but
        // no more than once per `DIFF_REBUILD_THROTTLE`. The rebuild is the
        // one expensive step in the loop; throttling it keeps the loop
        // responsive during a resize so held-key repeats don't pile into a
        // backlog that coasts after release.
        let mut rebuild_pending = diff_width != state.cached_width && diff_width > 0;
        if rebuild_pending && last_rebuild.elapsed() >= DIFF_REBUILD_THROTTLE {
            let view = build_view(
                &model.files,
                &model.diffs,
                &state.display_order,
                diff_width,
                theme,
            );
            state.diff_lines = view.lines;
            state.file_offsets = view.file_offsets;
            state.cached_width = diff_width;
            last_rebuild = Instant::now();
            rebuild_pending = false;
            // Clamp scroll to the new content length.
            let max = state.max_diff_scroll(diff_viewport);
            if state.diff_scroll > max {
                state.diff_scroll = max;
            }
        }

        // Pick the poll timeout: a deferred rebuild or a pending reload
        // both want a short wake; an idle loop uses the long timeout.
        let timeout = if rebuild_pending {
            DIFF_REBUILD_THROTTLE
        } else if let Some(since) = dirty_since {
            DEBOUNCE_DELAY.saturating_sub(since.elapsed())
        } else {
            POLL_TIMEOUT
        };
        let burst = read_event_burst(timeout)?;
        let cmd = apply_events(&mut state, burst, diff_viewport, sidebar_viewport);
        if cmd == AppCommand::Quit {
            break;
        }

        // Drain working-tree notifications, keeping only events that could
        // change the diff (a tracked/untracked, non-`.git/` path). Coalesce
        // the burst into one reload via the debounce window.
        while let Ok(paths) = notify_rx.try_recv() {
            if should_reload(&source, &paths) {
                dirty_since.get_or_insert_with(Instant::now);
            }
        }
        let reload_width = if diff_width > 0 {
            diff_width
        } else {
            state.cached_width
        };
        if dirty_since.is_some_and(|since| since.elapsed() >= DEBOUNCE_DELAY) {
            if let DiffSource::WorkingTree(repo) = source {
                reload_working_tree(
                    &mut state,
                    &mut model,
                    repo,
                    theme,
                    reload_width,
                    diff_viewport,
                )?;
                last_rebuild = Instant::now();
            }
            dirty_since = None;
        }
    }

    Ok(())
}

/// Build the sidebar from a model plus per-file delta counts.
fn build_sidebar(model: &Model, theme: &Theme) -> Sidebar {
    let sidebar_files: Vec<SidebarFile<'_>> = model
        .files
        .iter()
        .zip(model.diffs.iter())
        .map(|(f, d)| {
            let (added, deleted) = count_deltas(d);
            SidebarFile {
                file: &f.file,
                added,
                deleted,
            }
        })
        .collect();
    Sidebar::build(&sidebar_files, theme)
}

/// Whether a batch of changed `paths` warrants a working-tree reload.
///
/// Only [`DiffSource::WorkingTree`] reloads. A path counts when it is
/// neither inside `.git/` (git's constant index/lock churn) nor
/// gitignored (ignored files never appear in `working_tree_diff`, so a
/// change there can't alter the diff). Fails open via
/// [`git::Repo::is_ignored`], so a real change is never missed.
fn should_reload(source: &DiffSource<'_>, paths: &[PathBuf]) -> bool {
    let DiffSource::WorkingTree(repo) = source else {
        return false;
    };
    paths
        .iter()
        .any(|path| !is_git_internal(path) && !repo.is_ignored(path))
}

/// Whether `path` lies inside a `.git` directory.
fn is_git_internal(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

/// Re-diff the working tree and rebuild the view in place, preserving the
/// selected file by path. Captures the current selection's path from the
/// old `model`, builds a fresh model from `repo.working_tree_diff()`,
/// applies it via [`reload_view`], then swaps `model` to the new value.
fn reload_working_tree(
    state: &mut ViewState,
    model: &mut Model,
    repo: &git::Repo,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) -> Result<(), String> {
    let prev_path = state
        .sidebar
        .nearest_file_index()
        .and_then(|idx| model.files.get(idx))
        .map(|f| display_path(&f.file).to_string());
    let input = repo.working_tree_diff()?;
    let new_model = build_model(&input, Some(repo))?;
    reload_view(
        state,
        &new_model,
        prev_path.as_deref(),
        theme,
        width,
        diff_viewport,
    );
    *model = new_model;
    Ok(())
}

/// Rebuild the sidebar and diff view from `model`, preserving the user's
/// navigation state. Selection is restored by `prev_path` (index-based
/// restore would break when files are added or removed); when the file is
/// gone the fresh sidebar's default (first file) stands. Focus, sidebar
/// width, help visibility, and wheel state live on `state` and are left
/// untouched. Scroll is clamped to the new range then snapped to the
/// restored selection.
fn reload_view(
    state: &mut ViewState,
    model: &Model,
    prev_path: Option<&str>,
    theme: &Theme,
    width: usize,
    diff_viewport: usize,
) {
    let sidebar = build_sidebar(model, theme);
    let display_order = sidebar.display_order();
    let view = build_view(&model.files, &model.diffs, &display_order, width, theme);

    state.diff_lines = view.lines;
    state.file_offsets = view.file_offsets;
    state.display_order = display_order;
    state.sidebar = sidebar;
    state.cached_width = width;

    if let Some(path) = prev_path
        && let Some(idx) = model
            .files
            .iter()
            .position(|f| display_path(&f.file) == path)
    {
        state.sidebar.select_file_index(idx, diff_viewport);
    }

    let min = state.min_diff_scroll();
    let max = state.max_diff_scroll(diff_viewport);
    state.diff_scroll = state.diff_scroll.clamp(min, max.max(min));
    state.snap_diff_to_selected_file(diff_viewport);
}

/// Apply a whole burst of input events to `state`, stopping early on
/// `Quit`. Draining the queue per frame (see [`read_event_burst`]) collapses
/// the burst into a single redraw.
///
/// Sidebar-resize keys (`<`/`>`) are additionally coalesced to a single
/// step per burst. Holding the key fires OS auto-repeat, which buffers a
/// backlog of presses; applying every one would overshoot and keep growing
/// the sidebar after the key is released as the backlog drains. One step
/// per burst makes a hold grow at a steady, frame-paced rate that stops
/// within one frame of release. Mouse drag needs no such guard: it sets an
/// absolute width, so the last event in the burst already wins.
fn apply_events(
    state: &mut ViewState,
    events: impl IntoIterator<Item = Event>,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    let mut resized = false;
    for event in events {
        if is_resize_key(&event) {
            if resized {
                continue;
            }
            resized = true;
        }
        if dispatch_event(event, diff_viewport, sidebar_viewport, state) == AppCommand::Quit {
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

fn dispatch_event(
    event: Event,
    diff_viewport: usize,
    sidebar_viewport: usize,
    state: &mut ViewState,
) -> AppCommand {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(state, key.code, diff_viewport, sidebar_viewport)
        }
        Event::Mouse(mouse) => handle_mouse(state, mouse, diff_viewport, sidebar_viewport),
        Event::Resize(_, _) => AppCommand::Continue,
        _ => AppCommand::Continue,
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &mut ViewState, theme: &Theme) {
    let area = frame.area();

    let sw = state.sidebar_pref.effective(area.width);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sw), Constraint::Min(10)])
        .split(area);
    state.sidebar_rect = cols[0];
    state.diff_rect = cols[1];
    draw_sidebar(frame, cols[0], state, theme);
    draw_diff(frame, cols[1], state, theme);

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
    let color = pane_border_color(state.focus == Focus::Diff, theme);

    // After a reload that reverted/committed every change there are no
    // files: render a centered empty state rather than a blank pane.
    if state.display_order.is_empty() {
        let block = pane_block_with_footer("─[2]─Diff─", color, None);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let msg = Paragraph::new("No local changes.")
            .style(Style::default().fg(rgb_to_color(theme.muted)))
            .alignment(Alignment::Center);
        let mid = inner.height / 2;
        let line = Rect {
            x: inner.x,
            y: inner.y.saturating_add(mid),
            width: inner.width,
            height: 1.min(inner.height),
        };
        frame.render_widget(msg, line);
        return;
    }

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let viewport = inner.height as usize;
    let range = state.visible_diff_range();
    let scroll = state.diff_scroll.clamp(range.start, range.end);
    let end = scroll.saturating_add(viewport.max(1)).min(range.end);
    let visible: Vec<Line<'static>> = state.diff_lines[scroll..end].to_vec();

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

    fn make_state(files: &[ResolvedFile]) -> ViewState {
        let diffs = precompute_diffs(files);
        let sidebar_files: Vec<SidebarFile<'_>> = files
            .iter()
            .zip(diffs.iter())
            .map(|(f, d)| {
                let (added, deleted) = count_deltas(d);
                SidebarFile {
                    file: &f.file,
                    added,
                    deleted,
                }
            })
            .collect();
        let sidebar =
            Sidebar::build_with_icons(&sidebar_files, &theme(), crate::sidebar::IconMode::Off);
        let display_order = sidebar.display_order();
        let view = build_view(files, &diffs, &display_order, 80, &theme());
        ViewState::new(view, sidebar, display_order, 80, Preference::seeded(200))
    }

    #[test]
    fn build_view_emits_file_header_and_hunk_for_one_file() {
        let f = file_diff("foo.txt");
        let resolved = vec![ResolvedFile {
            file: f,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
                file: a,
                before: "old_alpha\n".to_string(),
                after: "new_alpha\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "old_beta\n".to_string(),
                after: "new_beta\n".to_string(),
            },
            ResolvedFile {
                file: c,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
            file: f,
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
        let Err(err) = resolve(parsed, None) else {
            panic!("resolve should fail on missing blob");
        };
        assert!(err.contains("missing index blob"), "got: {err}");
        assert!(err.contains("foo.txt"), "got: {err}");
    }

    #[test]
    fn handle_key_grow_and_shrink_sidebar() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let initial = state.sidebar_pref.effective(200);
        handle_key(&mut state, KeyCode::Char('>'), 4, 4);
        assert!(state.sidebar_pref.effective(200) > initial);
        handle_key(&mut state, KeyCode::Char('<'), 4, 4);
        assert_eq!(state.sidebar_pref.effective(200), initial);
    }

    #[test]
    fn handle_key_shrink_sidebar_floors_at_min() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        // Shrink well past the floor; effective width must settle at the
        // minimum (24) and never go lower.
        for _ in 0..20 {
            handle_key(&mut state, KeyCode::Char('<'), 4, 4);
        }
        let floored = state.sidebar_pref.effective(200);
        handle_key(&mut state, KeyCode::Char('<'), 4, 4);
        assert_eq!(state.sidebar_pref.effective(200), floored);
    }

    fn key_press(code: KeyCode) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ))
    }

    #[test]
    fn apply_events_coalesces_repeated_resize_keys() {
        // A burst of auto-repeated `>` (as when the key is held) must
        // grow the sidebar by a single step, not one per repeat.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let initial = state.sidebar_pref.effective(200);
        let burst = vec![
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
            key_press(KeyCode::Char('>')),
        ];
        apply_events(&mut state, burst, 4, 4);
        // One step (4 cols) per burst, not one per repeat.
        assert_eq!(state.sidebar_pref.effective(200), initial + 4);
    }

    #[test]
    fn apply_events_applies_non_resize_keys_each() {
        // Scroll keys are not coalesced: each repeat advances the diff.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        state.focus = Focus::Diff;
        let burst = vec![key_press(KeyCode::Char('j')), key_press(KeyCode::Char('j'))];
        apply_events(&mut state, burst, 2, 2);
        assert_eq!(state.diff_scroll, 2);
    }

    fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn make_state_with_rects(files: &[ResolvedFile]) -> ViewState {
        let mut state = make_state(files);
        state.sidebar_rect = Rect::new(0, 0, 38, 20);
        state.diff_rect = Rect::new(38, 0, 82, 20);
        state
    }

    #[test]
    fn pane_at_returns_correct_focus_review() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
    fn sidebar_burst_scroll_moves_one_row_per_tick() {
        // A single physical wheel tick fans out into a burst of events; the
        // shared WheelScroll collapses one quota's worth of events into a
        // single selection move, so the sidebar steps slowly like the traces
        // lists rather than jumping several rows per tick.
        let files: Vec<FileDiff> = (0..6).map(|i| file_diff(&format!("f{i}.txt"))).collect();
        let resolved: Vec<ResolvedFile> = files
            .into_iter()
            .map(|f| ResolvedFile {
                file: f,
                before: "a\n".to_string(),
                after: "b\n".to_string(),
            })
            .collect();
        let mut state = make_state_with_rects(&resolved);
        let initial = state.sidebar.selected();

        let burst = vec![
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 5)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 5)),
            Event::Mouse(make_mouse(MouseEventKind::ScrollDown, 5, 5)),
        ];
        apply_events(&mut state, burst, 18, 18);
        assert_eq!(state.sidebar.selected(), initial + 1);
    }

    #[test]
    fn scroll_on_diff_scrolls_content() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
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
            file: f,
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
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
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
    fn is_on_divider_matches_border_columns() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state_with_rects(&resolved);
        // sidebar_rect = (0,0,38,20): divider columns are 37 and 38.
        assert!(is_on_divider(&state, 37));
        assert!(is_on_divider(&state, 38));
        assert!(!is_on_divider(&state, 5));
        assert!(!is_on_divider(&state, 60));
    }

    #[test]
    fn is_on_divider_false_when_sidebar_hidden() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.sidebar_rect = Rect::default();
        assert!(!is_on_divider(&state, 0));
    }

    #[test]
    fn divider_press_starts_drag_without_selecting() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state_with_rects(&resolved);
        let selected = state.sidebar.selected();
        let focus = state.focus;

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 37, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.dragging_divider);
        assert_eq!(
            state.sidebar.selected(),
            selected,
            "divider press must not select"
        );
        assert_eq!(state.focus, focus, "divider press must not change focus");
    }

    #[test]
    fn divider_drag_resizes_and_release_ends() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);

        handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 37, 5),
            18,
            18,
        );
        // Drag right to column 50 -> width 51.
        handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5),
            18,
            18,
        );
        assert_eq!(state.sidebar_pref.effective(200), 51);
        // Drag far left -> floored at the minimum (24).
        handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Drag(MouseButton::Left), 2, 5),
            18,
            18,
        );
        assert_eq!(state.sidebar_pref.effective(200), 24);
        // Release ends the drag.
        handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Up(MouseButton::Left), 2, 5),
            18,
            18,
        );
        assert!(!state.dragging_divider);
    }

    #[test]
    fn non_divider_press_does_not_start_drag() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(!state.dragging_divider);
    }

    #[test]
    fn mouse_swallowed_while_help_visible() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
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
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Sidebar;

        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 200, 200);
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    // --- model / reload ---------------------------------------------

    /// A resolved file with distinct before/after so its diff is non-empty.
    fn resolved(path: &str) -> ResolvedFile {
        ResolvedFile {
            file: file_diff(path),
            before: format!("{path} old\n"),
            after: format!("{path} new\n"),
        }
    }

    fn model_of(paths: &[&str]) -> Model {
        let files: Vec<ResolvedFile> = paths.iter().map(|p| resolved(p)).collect();
        let diffs = precompute_diffs(&files);
        Model { files, diffs }
    }

    /// Input index of the file owning the current sidebar selection.
    fn selected_path(state: &ViewState, model: &Model) -> Option<String> {
        state
            .sidebar
            .nearest_file_index()
            .and_then(|i| model.files.get(i))
            .map(|f| display_path(&f.file).to_string())
    }

    #[test]
    fn reload_preserves_selection_by_path() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt (input index 1)
        let prev = selected_path(&state, &m1);
        assert_eq!(prev.as_deref(), Some("b.txt"));

        // New model inserts a file before b.txt, shifting its index.
        let m2 = model_of(&["a.txt", "aa.txt", "b.txt", "c.txt"]);
        reload_view(&mut state, &m2, prev.as_deref(), &theme(), 80, 4);

        assert_eq!(selected_path(&state, &m2).as_deref(), Some("b.txt"));
        // The diff pane is filtered to the restored file.
        let range = state.visible_diff_range();
        assert_eq!(line_text(&state.diff_lines[range.start]), "b.txt");
        assert!(range.contains(&state.diff_scroll) || state.diff_scroll == range.start);
    }

    #[test]
    fn reload_to_empty_model_renders_empty_state() {
        let m1 = model_of(&["a.txt", "b.txt"]);
        let mut state = make_state(&m1.files);
        let empty = Model {
            files: Vec::new(),
            diffs: Vec::new(),
        };
        reload_view(&mut state, &empty, Some("a.txt"), &theme(), 80, 4);

        assert!(state.diff_lines.is_empty());
        assert_eq!(state.visible_diff_range(), 0..0);
        assert_eq!(sidebar_footer(&state), None);
        assert_eq!(diff_footer(&state), None);
    }

    #[test]
    fn reload_clamps_selection_when_file_disappears() {
        let m1 = model_of(&["a.txt", "b.txt", "c.txt"]);
        let mut state = make_state(&m1.files);
        state.sidebar.select_file_index(1, 4); // b.txt

        // b.txt is gone (reverted/committed); selection must clamp.
        let m2 = model_of(&["a.txt", "c.txt"]);
        reload_view(&mut state, &m2, Some("b.txt"), &theme(), 80, 4);

        let path = selected_path(&state, &m2);
        assert!(
            matches!(path.as_deref(), Some("a.txt") | Some("c.txt")),
            "selection should clamp to a surviving file, got {path:?}"
        );
    }

    // --- working-tree integration -----------------------------------

    fn init_repo(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        repo
    }

    fn stage_all(repo: &git2::Repository) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
    }

    fn commit_index(repo: &git2::Repository, msg: &str) {
        let mut index = repo.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap();
    }

    #[test]
    fn build_model_from_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Stage the change so the post-image blob is in the ODB.
        std::fs::write(dir.path().join("a.txt"), "world\n").unwrap();
        stage_all(&repo);

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let input = wrapper.working_tree_diff().unwrap();
        let model = build_model(&input, Some(&wrapper)).unwrap();

        assert_eq!(model.files.len(), 1);
        assert_eq!(display_path(&model.files[0].file), "a.txt");
        assert_eq!(model.files[0].before, "hello\n");
        assert_eq!(model.files[0].after, "world\n");
        assert!(!model.diffs[0].hunks().is_empty());
    }

    #[test]
    fn should_reload_filters_ignored_and_git_internal() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let wrapper = git::Repo::discover_at(dir.path()).unwrap();
        let source = DiffSource::WorkingTree(&wrapper);

        assert!(should_reload(&source, &[dir.path().join("src/main.rs")]));
        assert!(!should_reload(
            &source,
            &[dir.path().join("node_modules/x.js")]
        ));
        assert!(!should_reload(
            &source,
            &[dir.path().join(".git/index.lock")]
        ));
        // A batch with at least one real path still reloads.
        assert!(should_reload(
            &source,
            &[
                dir.path().join(".git/index"),
                dir.path().join("src/main.rs"),
            ]
        ));
        // A static source never reloads.
        assert!(!should_reload(
            &DiffSource::Static,
            &[dir.path().join("src/main.rs")]
        ));
    }

    #[test]
    fn is_git_internal_detects_dot_git() {
        assert!(is_git_internal(Path::new("/repo/.git/index")));
        assert!(is_git_internal(Path::new(".git/HEAD")));
        assert!(!is_git_internal(Path::new("/repo/src/main.rs")));
    }
}
