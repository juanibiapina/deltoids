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
//! path), sidebar width, scroll, and the help popup. Bare mode also
//! *starts* in the "no local changes" empty state when the tree is
//! clean, repopulating on the first edit; reverting or committing every
//! change drops back to that empty state instead of exiting. The
//! filesystem watcher keeps edit latency low, but it ignores `.git/`
//! churn, so commits and external `git add`/`reset`/checkout/branch
//! switches are caught by a periodic poll of `git diff HEAD` (every
//! ~1s); the poll reloads only when that output actually changes.
//! Gitignored paths never trigger a reload. Piped-diff mode stays
//! static: stdin is closed, so there is nothing to re-read.
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
//! [`help::HELP_KEYS`]).
//!
//! ## Module layout
//!
//! The TUI is split by change axis (the pane/feature that changes
//! together). This file is the shell: it owns the entry point, the event
//! loop, event routing to panes, layout, the divider drag, and the
//! cross-pane coordination (snapping the diff to the sidebar selection).
//! Each pane owns its full vertical slice (state + input + render):
//!
//! - [`model`] — the data axis: parse/resolve/diff.
//! - [`diff_pane`] — the diff pane's state, scroll math, keys, render.
//! - [`sidebar_pane`] — the sidebar's build, keys, render, footer.
//! - [`help`] — the help popup.
//! - [`reload`] — the working-tree watcher and in-place rebuild.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};

use deltoids::{Theme, git};

use crate::events::read_event_burst;
use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};
use crate::sidebar::Sidebar;
use crate::sidebar_width::{self, Preference};
use crate::terminal::TerminalSession;

mod diff_pane;
mod help;
mod model;
mod reload;
mod sidebar_pane;
#[cfg(test)]
mod test_support;

use diff_pane::{DiffPane, SCROLL_STEP_LARGE, SCROLL_STEP_SMALL, build_view};
use model::{DiffSource, Model, build_model};
use reload::{reload_working_tree, should_reload, spawn_watcher};
use sidebar_pane::build_sidebar;

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

/// Idle poll timeout for the event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Debounce window for working-tree change events. A burst of file-system
/// notifications (an editor save touches several files, git churns the
/// index) collapses into one reload once this much wall-clock has passed
/// since the first event. Matches the traces TUI.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);
/// Minimum wall-clock gap between polls of `git diff HEAD`. The
/// filesystem watcher ignores `.git/` churn, so commits, external
/// `git add`/`reset`, checkouts, and branch switches are only noticed by
/// re-running the diff. Gated to one cheap `working_tree_diff` per
/// interval and deduped against the last input, so an unchanged tree
/// costs one git call and no view rebuild. This interval sets the
/// worst-case delay before a commit clears the diff; ~1s trades a little
/// latency for staying cheap on large repos.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(1);
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

    run_tui(model, input, source, &theme)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Diff,
}

/// The shell's owned state. The per-pane sub-state lives in
/// [`DiffPane`] and [`Sidebar`]; the remaining fields are shell-level
/// concerns (focus, help visibility, divider drag, layout rects, sidebar
/// sizing, wheel smoothing).
pub(crate) struct ViewState {
    /// Diff pane vertical slice.
    diff: DiffPane,
    /// Sidebar pane state: rows, selection, scroll.
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
    fn new(diff: DiffPane, sidebar: Sidebar, sidebar_pref: Preference) -> Self {
        Self {
            diff,
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

    /// Window of the diff that should be visible right now. Cross-pane
    /// coordination: it queries the sidebar's selection range and hands
    /// it to the diff pane, which owns the filtering math.
    #[cfg(test)]
    fn visible_diff_range(&self) -> std::ops::Range<usize> {
        self.diff
            .visible_range(self.sidebar.selection_display_range())
    }

    /// Sync the diff pane's scroll to the file the sidebar is pointing
    /// at. Cross-pane coordination owned by the shell: it queries the
    /// sidebar for the nearest file index and selection range, then asks
    /// the diff pane to snap. Neither pane reaches into the other.
    fn snap_diff_to_selected_file(&mut self, viewport: usize) {
        let dr = self.sidebar.selection_display_range();
        let Some(file_idx) = self.sidebar.nearest_file_index() else {
            return;
        };
        self.diff.snap_to_file(file_idx, viewport, dr);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppCommand {
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
        return help::handle_key_help(state, key);
    }
    match key {
        KeyCode::Char('?') => {
            state.help_visible = true;
            AppCommand::Continue
        }
        KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
        KeyCode::Tab | KeyCode::BackTab => {
            // Two panes: BackTab is the same toggle as Tab.
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
        // a long diff. This is the documented cross-cutting binding: the
        // shell routes it straight to the diff pane.
        KeyCode::Char('J') => {
            let dr = state.sidebar.selection_display_range();
            state
                .diff
                .scroll_by(SCROLL_STEP_LARGE as isize, diff_viewport, dr);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            let dr = state.sidebar.selection_display_range();
            state
                .diff
                .scroll_by(-(SCROLL_STEP_LARGE as isize), diff_viewport, dr);
            AppCommand::Continue
        }
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
        // Remaining nav keys route to the focused pane. A sidebar move
        // also snaps the diff (cross-pane coordination owned here).
        other => {
            match state.focus {
                Focus::Sidebar => {
                    if sidebar_pane::handle_key(&mut state.sidebar, other, sidebar_viewport) {
                        state.snap_diff_to_selected_file(diff_viewport);
                    }
                }
                Focus::Diff => {
                    let dr = state.sidebar.selection_display_range();
                    state.diff.handle_key(other, diff_viewport, dr);
                }
            }
            AppCommand::Continue
        }
    }
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
                let dr = state.sidebar.selection_display_range();
                state
                    .diff
                    .scroll_by((steps * SCROLL_STEP_SMALL) as isize, diff_viewport, dr);
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
                let dr = state.sidebar.selection_display_range();
                state
                    .diff
                    .scroll_by(-((steps * SCROLL_STEP_SMALL) as isize), diff_viewport, dr);
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

#[allow(clippy::too_many_lines)]
fn run_tui(
    mut model: Model,
    initial_input: String,
    source: DiffSource<'_>,
    theme: &Theme,
) -> Result<(), String> {
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
    let diff = DiffPane::new(view, display_order, initial_diff_width);
    let mut state = ViewState::new(diff, sidebar, sidebar_pref);

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
    // The diff text the current model was built from. The poll compares
    // fresh `working_tree_diff` output against this to skip rebuilds when
    // nothing changed.
    let mut last_input = initial_input;
    let mut last_poll = Instant::now();
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
        let mut rebuild_pending = diff_width != state.diff.cached_width && diff_width > 0;
        if rebuild_pending && last_rebuild.elapsed() >= DIFF_REBUILD_THROTTLE {
            let view = build_view(
                &model.files,
                &model.diffs,
                &state.diff.display_order,
                diff_width,
                theme,
            );
            state.diff.diff_lines = view.lines;
            state.diff.file_offsets = view.file_offsets;
            state.diff.cached_width = diff_width;
            last_rebuild = Instant::now();
            rebuild_pending = false;
            // Clamp scroll to the new content length.
            let dr = state.sidebar.selection_display_range();
            let max = state.diff.max_scroll(dr, diff_viewport);
            if state.diff.diff_scroll > max {
                state.diff.diff_scroll = max;
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
        // Poll git on a steady interval to catch state changes the
        // filesystem watcher filters out (commits and other `.git/`
        // writes). Route it through the same debounce; the reload dedups
        // on input, so a tick with no real change is cheap.
        if matches!(source, DiffSource::WorkingTree(_)) && last_poll.elapsed() >= GIT_POLL_INTERVAL
        {
            dirty_since.get_or_insert_with(Instant::now);
            last_poll = Instant::now();
        }
        let reload_width = if diff_width > 0 {
            diff_width
        } else {
            state.diff.cached_width
        };
        if dirty_since.is_some_and(|since| since.elapsed() >= DEBOUNCE_DELAY) {
            if let DiffSource::WorkingTree(repo) = source
                && reload_working_tree(
                    &mut state,
                    &mut model,
                    repo,
                    theme,
                    reload_width,
                    diff_viewport,
                    &mut last_input,
                )?
            {
                last_rebuild = Instant::now();
            }
            dirty_since = None;
        }
    }

    Ok(())
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

    let dr = state.sidebar.selection_display_range();
    sidebar_pane::draw_sidebar(
        frame,
        cols[0],
        &state.sidebar,
        &state.diff.display_order,
        state.focus == Focus::Sidebar,
        theme,
    );
    state
        .diff
        .render(frame, cols[1], state.focus == Focus::Diff, theme, dr);

    if state.help_visible {
        help::draw_help_popup(frame, area, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::review::test_support::*;
    use model::ResolvedFile;

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
        assert_eq!(state.diff.diff_scroll, 2);
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
}
