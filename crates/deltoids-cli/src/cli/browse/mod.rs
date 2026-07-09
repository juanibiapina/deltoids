//! The unified scrolling TUI shared by `deltoids review` and
//! `deltoids traces`.
//!
//! The left panel cycles, lazygit-style, between two **modes**:
//!
//! - **Files** ([`files::FilesMode`]): a file-tree sidebar plus the
//!   working-tree (or piped) diff.
//! - **Traces** ([`traces::TracesMode`]): an entries list and a traces
//!   list stacked on the left, the selected entry's detail/diff on the
//!   right.
//!
//! `]` cycles to the next mode and `[` to the previous; clicking a tab
//! label switches directly to that mode. The right pane follows whichever
//! mode is active. The active mode's top-left panel title shows a
//! `Files - Traces` tab strip with the active label highlighted.
//! Both subcommands open this same TUI, seeded with a different starting
//! mode: `review` → Files, `traces` → Traces.
//!
//! ## Module layout
//!
//! This file is the mode-agnostic shell: it owns the terminal, the event
//! loop, the one draggable divider between the left column and the right
//! pane, the shared sidebar width, the `<`/`>` resize (per-burst
//! coalesced), the help popup, the `[`/`]` mode toggle, and the
//! live-reload orchestration. Everything that varies between the two
//! modes lives behind [`mode::Mode`]; the two adapters live in
//! [`files`] and [`traces`].
//!
//! Each frame the shell also derives a [`mode::DrawBudget`] from whether
//! input is still streaming: `Fast` while a navigation key is held (an
//! input burst is non-empty), `Full` once it settles (an empty burst =
//! poll timeout). Modes use this to defer expensive rendering — Traces
//! mode skips highlighting an unseen entry's diff on `Fast` frames, so
//! holding `j` stays smooth over large edits. While input streams the
//! poll timeout shrinks to `SETTLE_TIMEOUT` so the settled `Full` frame
//! lands promptly after release.
//!
//! Live reload keeps both modes resident: both watchers are armed once at
//! startup, so toggling is instant. The shell drains both receivers each
//! loop, marks the owning mode dirty, reloads the **active** mode eagerly
//! after a debounce, and reloads the **inactive** mode lazily the moment
//! it becomes active.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_block_with_tabs, rgb_to_color};

use crate::events::read_event_burst;
use crate::sidebar_width::{self, Preference};
use crate::terminal::TerminalSession;

mod command;
pub mod files;
mod help;
pub mod mode;
mod suspend;
pub mod traces;
mod watch;

use command::{CustomCommand, load_commands};
use files::FilesMode;
use mode::{AppCommand, CustomRun, DrawBudget, Mode, ReloadViewport, TAB_LABELS, TabStrip};
use traces::TracesMode;

/// Active-mode index for the Files panel.
pub const FILES_MODE: usize = 0;
/// Active-mode index for the Traces panel.
pub const TRACES_MODE: usize = 1;
/// Number of left-panel modes the shell cycles through.
pub const MODE_COUNT: usize = TAB_LABELS.len();

/// Pick the starting mode: Files when the working tree has local changes,
/// otherwise Traces. Outside a repo (or on any git error) Traces, since
/// there is no working-tree diff to show.
pub fn smart_initial_mode() -> usize {
    if files::working_tree_has_changes() {
        FILES_MODE
    } else {
        TRACES_MODE
    }
}

/// Idle poll timeout for the event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Debounce window for change events: a burst of notifications collapses
/// into one reload once this much wall-clock has passed since the first.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);
/// Minimum wall-clock gap between git polls (Files mode catches commits
/// and other `.git/` writes the filesystem watcher filters out).
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Poll timeout while input is streaming (a navigation key is held). Kept
/// short so the first idle frame after release lands quickly and rebuilds
/// any diff deferred during the hold.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(80);

/// Run the headless (non-TTY) scripted render path for Traces mode.
pub fn run_traces_scripted() -> Result<(), String> {
    traces::run_scripted_for_cwd()
}

/// Open the unified TUI seeded with `active_mode` ([`FILES_MODE`] or
/// [`TRACES_MODE`]).
pub fn run(active_mode: usize) -> Result<(), String> {
    let theme = Theme::load();
    let _session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;

    let total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    let sidebar_pref = Preference::seeded(total_width);
    let sidebar_w = sidebar_pref.effective(total_width);
    let initial_diff_width = sidebar_width::diff_pane_width(sidebar_w, total_width);

    // Both modes start as cheap empty placeholders. Neither is built yet,
    // so the loop's first iteration draws a loading frame and then builds
    // the active mode (the same path a toggle takes). Startup shows a
    // loading state instead of a blank screen during the (possibly slow)
    // build. The inactive mode stays a placeholder until first activated,
    // so a session that never opens it never pays its build/watcher cost.
    let mut modes: [Box<dyn Mode>; MODE_COUNT] = [
        Box::new(FilesMode::empty(&theme, initial_diff_width)),
        Box::new(TracesMode::empty()),
    ];

    let mut shell = Shell::new(active_mode, sidebar_pref, total_width);
    shell.commands = load_commands();

    loop {
        let active = shell.active;
        let help_visible = shell.help_visible;
        let pref = shell.sidebar_pref;
        let commands = &shell.commands;
        // When the active mode hasn't been built yet (just toggled to), draw
        // a loading frame instead. The tab strip already shows the switch,
        // so the UI feels responsive while the (possibly slow) build runs.
        let building = !shell.built[active];
        let budget = shell.draw_budget();
        let area = terminal
            .draw(|frame| {
                let area = frame.area();
                let sw = pref.effective(area.width);
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(sw), Constraint::Min(10)])
                    .split(area);
                if building {
                    draw_loading(frame, cols[0], cols[1], TabStrip { active }, &theme);
                } else {
                    modes[active].draw(
                        frame,
                        cols[0],
                        cols[1],
                        TabStrip { active },
                        &theme,
                        budget,
                    );
                }
                if help_visible {
                    help::draw_help_popup(frame, area, &theme, commands);
                }
            })
            .map_err(|err| format!("failed to render screen: {err}"))?
            .area;

        // Recompute the layout split from the drawn area: the closure can't
        // return a value, so derive the divider rect and viewports here.
        shell.total_width = area.width;
        let sw = shell.sidebar_pref.effective(area.width);
        shell.left_rect = Rect {
            x: area.x,
            y: area.y,
            width: sw,
            height: area.height,
        };
        let pane_viewport = area.height.saturating_sub(2) as usize;
        let vp = ReloadViewport {
            left_viewport: pane_viewport,
            right_viewport: pane_viewport,
            right_width: sidebar_width::diff_pane_width(sw, area.width),
        };

        // The loading frame is now on screen; build the mode for real, then
        // loop back to draw its content.
        if building {
            shell.build_active(&mut modes, vp, &theme);
            continue;
        }

        let timeout = shell.poll_timeout();
        let burst = read_event_burst(timeout)?;
        // An empty burst is a poll timeout: input has settled. A non-empty
        // burst means the user is actively navigating, so the next frame
        // draws Fast (deferring expensive rendering).
        shell.note_input(burst.is_empty());
        match shell.apply_events(&mut modes, burst, vp, &theme)? {
            AppCommand::Quit => break,
            // A custom command runs here, in the loop that owns the
            // terminal. Background: run without touching the terminal, so
            // the next draw is unchanged (no flicker). Subprocess: suspend,
            // hand the terminal to the child, restore. Errors are ignored
            // in v1 (the screen is intact / rebuilt regardless).
            AppCommand::Run(run) => {
                if run.subprocess {
                    let _ = suspend::run_foreground(&mut terminal, &run.command);
                } else {
                    let _ = suspend::run_background(&run.command);
                }
            }
            AppCommand::Continue => {}
        }

        // Drain armed watchers; mark the owning mode dirty.
        shell.drain_watchers(&mut modes);

        // Periodic git poll for any mode that wants it (Files working
        // tree). Routed through the same debounce; reload dedups.
        if shell.last_poll.elapsed() >= GIT_POLL_INTERVAL {
            shell.mark_poll_dirty();
            shell.last_poll = Instant::now();
        }

        // Reload the active mode eagerly once its debounce elapses (or
        // immediately after a toggle). The inactive mode stays dirty and
        // reloads on its next activation.
        shell.reload_active_if_due(&mut modes, vp, &theme)?;
    }

    Ok(())
}

/// The shell's owned, mode-agnostic state.
struct Shell {
    /// Active mode index ([`FILES_MODE`] / [`TRACES_MODE`]).
    active: usize,
    /// Shared sidebar width preference (one width for both modes).
    sidebar_pref: Preference,
    /// True while the left button is held on the pane divider.
    dragging_divider: bool,
    /// Whether the help popup is shown.
    help_visible: bool,
    /// Last-drawn left-column rect, for divider hit-testing.
    left_rect: Rect,
    /// Per-mode change receivers, armed lazily on first activation.
    receivers: [Option<Receiver<Vec<PathBuf>>>; MODE_COUNT],
    /// Whether each mode's watcher has been armed yet.
    armed: [bool; MODE_COUNT],
    /// Whether each mode wants the periodic git poll.
    needs_poll: [bool; MODE_COUNT],
    /// Whether each mode has been built for real yet (vs the startup
    /// empty placeholder).
    built: [bool; MODE_COUNT],
    /// Last-known terminal width, for sizing a lazily-built mode.
    total_width: u16,
    /// Per-mode dirty timestamps (first change of the current batch).
    dirty_since: [Option<Instant>; MODE_COUNT],
    /// Set on a mode toggle to force an immediate reload of the now-active
    /// mode if it is dirty.
    toggle_pending: bool,
    /// Last git poll.
    last_poll: Instant,
    /// Whether the most recent input burst was empty (a poll timeout). When
    /// false, the user is actively navigating and the next frame draws
    /// `Fast`; when true, it draws `Full`.
    input_idle: bool,
    /// User-configured custom key commands (from `config.toml`).
    commands: Vec<CustomCommand>,
}

impl Shell {
    fn new(active: usize, sidebar_pref: Preference, total_width: u16) -> Self {
        Self {
            active,
            sidebar_pref,
            dragging_divider: false,
            help_visible: false,
            left_rect: Rect::default(),
            receivers: [None, None],
            armed: [false, false],
            needs_poll: [false, false],
            built: [false, false],
            total_width,
            dirty_since: [None, None],
            toggle_pending: false,
            last_poll: Instant::now(),
            input_idle: true,
            commands: Vec::new(),
        }
    }

    /// Arm mode `index`'s watcher if it has not been armed yet. Idempotent
    /// across toggles, so a watcher is never re-armed.
    fn arm(&mut self, modes: &mut [Box<dyn Mode>; MODE_COUNT], index: usize) {
        if self.armed[index] {
            return;
        }
        self.receivers[index] = modes[index].watch();
        self.armed[index] = true;
    }

    /// Drain both watchers, marking the owning mode dirty for any batch
    /// that warrants a reload.
    fn drain_watchers(&mut self, modes: &mut [Box<dyn Mode>; MODE_COUNT]) {
        for index in 0..self.receivers.len() {
            let Some(rx) = self.receivers[index].take() else {
                continue;
            };
            self.drain_one(modes, index, &rx);
            self.receivers[index] = Some(rx);
        }
    }

    /// Drain one receiver, marking mode `index` dirty for any batch that
    /// warrants a reload.
    fn drain_one(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        index: usize,
        rx: &Receiver<Vec<PathBuf>>,
    ) {
        while let Ok(paths) = rx.try_recv() {
            if modes[index].should_reload(&paths) {
                self.dirty_since[index].get_or_insert_with(Instant::now);
            }
        }
    }

    /// Mark dirty every mode that wants a git poll this tick.
    fn mark_poll_dirty(&mut self) {
        for index in 0..self.needs_poll.len() {
            if self.needs_poll[index] {
                self.dirty_since[index].get_or_insert_with(Instant::now);
            }
        }
    }

    /// The draw budget for the current frame: `Full` when input has
    /// settled, `Fast` while a navigation key is streaming so modes can
    /// defer expensive rendering.
    fn draw_budget(&self) -> DrawBudget {
        if self.input_idle {
            DrawBudget::Full
        } else {
            DrawBudget::Fast
        }
    }

    /// Record whether the just-read input burst was empty (a poll timeout,
    /// i.e. input settled). Drives the next frame's [`Shell::draw_budget`].
    fn note_input(&mut self, burst_empty: bool) {
        self.input_idle = burst_empty;
    }

    /// Pick the loop's poll timeout: short while a reload is pending,
    /// otherwise the idle timeout.
    fn poll_timeout(&self) -> Duration {
        if let Some(since) = self.dirty_since[self.active] {
            return DEBOUNCE_DELAY.saturating_sub(since.elapsed());
        }
        if !self.input_idle {
            return SETTLE_TIMEOUT;
        }
        POLL_TIMEOUT
    }

    /// The two adjacent border columns forming the divider between the
    /// left column and the right pane. `None` when the left column has
    /// zero width.
    fn divider_columns(&self) -> Option<(u16, u16)> {
        if self.left_rect.width == 0 {
            return None;
        }
        let right_border = self.left_rect.right().saturating_sub(1);
        Some((right_border, right_border.saturating_add(1)))
    }

    fn is_on_divider(&self, col: u16) -> bool {
        matches!(self.divider_columns(), Some((a, b)) if col == a || col == b)
    }

    /// Cycle the active mode by one step (`forward` = `]`, else `[`),
    /// wrapping around, and arm an immediate lazy reload if the now-active
    /// mode is dirty.
    fn cycle(&mut self, forward: bool) {
        let next = if forward {
            (self.active + 1) % MODE_COUNT
        } else {
            (self.active + MODE_COUNT - 1) % MODE_COUNT
        };
        self.select_mode(next);
    }

    /// Make mode `index` active and arm an immediate lazy reload if it is
    /// dirty. Shared by keyboard cycling (`cycle`) and tab clicks. A
    /// no-op when `index` is already active, so re-selecting the current
    /// tab never sets a spurious reload.
    fn select_mode(&mut self, index: usize) {
        if index == self.active {
            return;
        }
        self.active = index;
        self.toggle_pending = true;
    }

    /// Build the active mode for real once its loading frame is on screen.
    /// No-op if already built. Arms its watcher and clears any stale
    /// dirty/toggle state, since a fresh build already reflects disk.
    fn build_active(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        vp: ReloadViewport,
        theme: &Theme,
    ) {
        let i = self.active;
        if self.built[i] {
            return;
        }
        let dw = if vp.right_width > 0 {
            vp.right_width
        } else {
            sidebar_width::diff_pane_width(
                self.sidebar_pref.effective(self.total_width),
                self.total_width,
            )
        };
        modes[i] = build_mode(i, theme, dw);
        self.built[i] = true;
        self.needs_poll[i] = modes[i].needs_git_poll();
        self.arm(modes, i);
        self.dirty_since[i] = None;
        self.toggle_pending = false;
    }

    /// Reload the active mode if its debounce has elapsed or a toggle
    /// just made it active while dirty.
    fn reload_active_if_due(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        vp: ReloadViewport,
        theme: &Theme,
    ) -> Result<(), String> {
        let active = self.active;
        let due = self.dirty_since[active].is_some_and(|s| s.elapsed() >= DEBOUNCE_DELAY);
        if due || self.toggle_pending {
            if self.dirty_since[active].is_some() {
                modes[active].reload(vp, theme)?;
                self.dirty_since[active] = None;
            }
            self.toggle_pending = false;
        }
        Ok(())
    }

    /// Handle a key already in the shell. Global bindings are consumed
    /// here; everything else routes to the active mode.
    fn handle_key(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        key: KeyCode,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        if self.help_visible {
            return help::handle_key_help(&mut self.help_visible, key);
        }
        match key {
            KeyCode::Char('?') => {
                self.help_visible = true;
                AppCommand::Continue
            }
            KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
            // `]` cycles to the next mode, `[` to the previous.
            KeyCode::Char(']') => {
                self.cycle(true);
                AppCommand::Continue
            }
            KeyCode::Char('[') => {
                self.cycle(false);
                AppCommand::Continue
            }
            KeyCode::Char('>') => {
                self.sidebar_pref.widen();
                AppCommand::Continue
            }
            KeyCode::Char('<') => {
                self.sidebar_pref.narrow();
                AppCommand::Continue
            }
            // Custom commands take priority over a mode's own keys (but not
            // over the shell globals above). A bound key with a selectable
            // file expands and bubbles up a Run request; with nothing
            // selected it is a silent no-op.
            KeyCode::Char(c) if self.command_for(c).is_some() => {
                let cmd = self.command_for(c).expect("checked by guard");
                match modes[self.active].selected_path() {
                    Some(path) => AppCommand::Run(CustomRun {
                        command: command::expand(&cmd.command, &path),
                        subprocess: cmd.subprocess,
                    }),
                    None => AppCommand::Continue,
                }
            }
            other => modes[self.active].handle_key(other, left_viewport, right_viewport),
        }
    }

    /// The custom command bound to `key`, if any.
    fn command_for(&self, key: char) -> Option<&CustomCommand> {
        self.commands.iter().find(|c| c.key == key)
    }

    /// Handle a mouse event. Divider drag is resolved here; everything
    /// else routes to the active mode.
    fn handle_mouse(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        mouse: MouseEvent,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        if self.help_visible {
            return AppCommand::Continue;
        }
        // A left-click on a tab label in the top-left panel title switches
        // to that mode, matching keyboard `[`/`]`. The strip sits on the
        // left column's top border row, one column in from its left edge.
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind
            && mouse.row == self.left_rect.y
            && let Some(index) = (TabStrip {
                active: self.active,
            })
            .hit_test(mouse.column, self.left_rect.x.saturating_add(1))
        {
            self.select_mode(index);
            return AppCommand::Continue;
        }
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging_divider = false;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging_divider => {
                self.sidebar_pref.set_from_divider(mouse.column);
                return AppCommand::Continue;
            }
            MouseEventKind::Down(MouseButton::Left) if self.is_on_divider(mouse.column) => {
                self.dragging_divider = true;
                return AppCommand::Continue;
            }
            _ => {}
        }
        modes[self.active].handle_mouse(mouse, left_viewport, right_viewport)
    }

    /// Apply a whole burst of input events, stopping early on `Quit`.
    /// Sidebar-resize keys (`<`/`>`) are coalesced to one step per burst.
    fn apply_events(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        events: impl IntoIterator<Item = Event>,
        vp: ReloadViewport,
        theme: &Theme,
    ) -> Result<AppCommand, String> {
        let mut resized = false;
        for event in events {
            // Coalesce sidebar-resize keys to one step per burst.
            if is_resize_key(&event) && std::mem::replace(&mut resized, true) {
                continue;
            }
            let cmd = self.dispatch(modes, event, vp);
            // A toggle may have armed a lazy reload mid-burst; service it
            // before the next draw so the now-active mode is fresh.
            self.reload_active_if_due(modes, vp, theme)?;
            // Bubble Quit and Run up to the run() loop (which owns the
            // terminal); keep draining the burst only on Continue.
            if cmd != AppCommand::Continue {
                return Ok(cmd);
            }
        }
        Ok(AppCommand::Continue)
    }

    /// Dispatch one input event to the global handlers / active mode.
    fn dispatch(
        &mut self,
        modes: &mut [Box<dyn Mode>; MODE_COUNT],
        event: Event,
        vp: ReloadViewport,
    ) -> AppCommand {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.handle_key(modes, key.code, vp.left_viewport, vp.right_viewport)
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(modes, mouse, vp.left_viewport, vp.right_viewport)
            }
            _ => AppCommand::Continue,
        }
    }
}

/// Render the loading frame for a not-yet-built active mode: the tab
/// strip in the top-left panel (so the toggle is visible) plus a centered
/// `Loading…` message in both columns.
fn draw_loading(
    frame: &mut ratatui::Frame<'_>,
    left: Rect,
    right: Rect,
    tabs: TabStrip,
    theme: &Theme,
) {
    let border = rgb_to_color(theme.border);
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let loading = |block| {
        Paragraph::new("Loading…")
            .style(muted)
            .alignment(Alignment::Center)
            .block(block)
    };
    frame.render_widget(
        loading(pane_block_with_tabs(
            tabs.title_line(border, theme),
            border,
            None,
        )),
        left,
    );
    frame.render_widget(loading(pane_block("─Diff─", border)), right);
}

/// Build a mode for real on first activation. Each mode folds its own
/// build failure into a renderable state (Files shows an error message;
/// Traces degrades to empty) so a toggle never aborts the session.
fn build_mode(index: usize, theme: &Theme, diff_width: usize) -> Box<dyn Mode> {
    match index {
        FILES_MODE => Box::new(FilesMode::build(theme, diff_width)),
        _ => Box::new(TracesMode::build().unwrap_or_else(|_| TracesMode::empty())),
    }
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

#[cfg(test)]
mod tests;
