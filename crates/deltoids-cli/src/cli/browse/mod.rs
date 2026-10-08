//! The scrolling TUI opened by `deltoids tui`.
//!
//! The left column is a file-tree sidebar and the right pane shows the
//! working-tree (or piped) diff. Both belong to [`files::FilesMode`].
//!
//! ## Module layout
//!
//! This file is the shell: it owns the terminal, the event loop, the one
//! draggable divider between the left column and the right pane, the
//! sidebar width, the `<`/`>` resize (per-burst coalesced), the help
//! popup, the theme picker, custom commands, and the live-reload
//! orchestration. Everything else lives behind [`mode::Mode`], whose
//! production adapter is [`files::FilesMode`]; the shell tests drive the
//! shell through a recording adapter.
//!
//! Each frame the shell also derives a [`mode::DrawBudget`] from whether
//! input is still streaming: `Fast` while a navigation key is held (an
//! input burst is non-empty), `Full` once it settles (an empty burst =
//! poll timeout). The mode uses this to defer expensive rendering. While
//! input streams the poll timeout shrinks to `SETTLE_TIMEOUT` so the
//! settled `Full` frame lands promptly after release.
//!
//! The mode loads and installs its watcher on the first loop iteration,
//! after a loading frame is on screen. The shell takes bounded
//! notification batches each loop and reloads the mode after a debounce.
//! Healthy watchers cause no periodic reloads; failures trigger explicit
//! watcher recovery or read retries. Focus loss defers refreshes,
//! recovery, and drawing while bounded watcher batches retain changes.
//! Idle timeouts draw only the single settled frame after fast rendering.

use std::io;
use std::time::{Duration, Instant};
use watch::ChangeReceiver;

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use deltoids::render_tui::{pane_block, pane_block_with_title_line, rgb_to_color};
use deltoids::{ChangeLayout, Theme};

use crate::events::read_event_burst;
use crate::sidebar_width::{self, Preference};
use crate::terminal::TerminalSession;

mod clipboard;
mod command;
mod comment_view;
mod comments;
mod diff_cursor;
mod diff_scrollbar;
pub mod files;
mod help;
pub mod mode;
mod suspend;
mod syntax_badge;
mod text;
mod theme_picker;
mod watch;

use command::{CustomCommand, load_commands};
use files::FilesMode;
use mode::{AppCommand, CustomRun, DrawBudget, Mode, ReloadViewport};
use theme_picker::{PickerAction, ThemePicker};

/// Idle poll timeout for the event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Debounce window for change events: a burst of notifications collapses
/// into one reload once this much wall-clock has passed since the first.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);
const READ_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_WATCH_RETRY_DELAY: Duration = Duration::from_secs(5);
/// Poll timeout while input is streaming (a navigation key is held). Kept
/// short so the first idle frame after release lands quickly and rebuilds
/// any diff deferred during the hold.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(80);
const RENDER_POLL_TIMEOUT: Duration = Duration::from_millis(8);

type BrowseTerminal = Terminal<CrosstermBackend<io::BufWriter<io::Stdout>>>;

fn buffered_backend() -> CrosstermBackend<io::BufWriter<io::Stdout>> {
    // Ratatui flushes after each frame. Buffer ANSI fragments so drawing does
    // not block input on hundreds of small writes to the terminal.
    CrosstermBackend::new(io::BufWriter::with_capacity(128 * 1024, io::stdout()))
}

/// Open the TUI on the working-tree diff.
pub fn run() -> Result<(), String> {
    let mut theme = Theme::load();
    let session = TerminalSession::enter()?;
    let backend = buffered_backend();
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;

    let total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    let sidebar_pref = Preference::seeded(total_width);
    let sidebar_w = sidebar_pref.effective(total_width);
    let initial_diff_width =
        diff_scrollbar::body_width(sidebar_width::diff_pane_width(sidebar_w, total_width));

    // The mode starts as a cheap empty placeholder, so the loop's first
    // iteration draws a loading frame and then builds it. Startup shows a
    // loading state instead of a blank screen during the (possibly slow)
    // build.
    let mut mode: Box<dyn Mode> = Box::new(FilesMode::empty(&theme, initial_diff_width));

    let mut shell = Shell::new(sidebar_pref, total_width, theme.syntax_theme_name.clone());
    shell.commands = load_commands();

    let mut vp = ReloadViewport::default();
    loop {
        if session.quit_requested() {
            break;
        }
        // Drain queued input before refreshes, including focus loss that
        // arrived while the previous synchronous operation was finishing.
        let queued = read_event_burst(Duration::ZERO)?;
        if !queued.is_empty() {
            shell.note_input(false);
            let cmd = shell.apply_events(&mut mode, queued, vp, &theme)?;
            if apply_app_command(cmd, &mut terminal, &mut mode, &mut shell, &mut theme) {
                break;
            }
        }
        shell.drain_watchers(&mut mode);
        shell.poll_background(&mut mode);
        shell.reload_if_due(&mut mode, vp, &theme)?;

        if shell.needs_redraw() {
            vp = draw_frame(&mut terminal, &mut mode, &mut shell, &theme)?;
            if !shell.built {
                shell.build(&mut mode, vp, &theme);
                continue;
            }
        }

        let pending = shell.poll_background(&mut mode);
        if shell.needs_redraw() {
            continue;
        }
        let timeout = if pending {
            shell.poll_timeout().min(RENDER_POLL_TIMEOUT)
        } else {
            shell.poll_timeout()
        };
        let burst = read_event_burst(timeout)?;
        shell.note_input(burst.is_empty());
        let cmd = shell.apply_events(&mut mode, burst, vp, &theme)?;
        if apply_app_command(cmd, &mut terminal, &mut mode, &mut shell, &mut theme) {
            break;
        }
    }

    // Restore the user's terminal before watcher teardown finishes.
    drop(session);
    Ok(())
}

fn draw_frame(
    terminal: &mut BrowseTerminal,
    mode: &mut Box<dyn Mode>,
    shell: &mut Shell,
    theme: &Theme,
) -> Result<ReloadViewport, String> {
    let help_visible = shell.help_visible;
    let pref = shell.sidebar_pref;
    let layout = shell.change_layout;
    // Until the mode is built, draw a loading frame so the UI responds
    // while the (possibly slow) build runs.
    let building = !shell.built;
    // Take the budget (a `&mut` op) before borrowing `commands`.
    let budget = shell.take_draw_budget();
    let commands = &shell.commands;
    let theme_picker = shell.theme_picker.as_ref();
    let active_theme = shell.syntax_theme.as_str();
    let refresh_error = shell.refresh_error();
    let area = terminal
        .draw(|frame| {
            let area = frame.area();
            let sw = pref.effective(area.width);
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(sw), Constraint::Min(10)])
                .split(area);
            if building {
                draw_loading(frame, cols[0], cols[1], theme);
            } else {
                mode.draw(frame, cols[0], cols[1], layout, theme, budget);
            }
            if help_visible {
                let commands: Vec<_> = commands
                    .iter()
                    .filter(|c| !mode.reserves_key(KeyCode::Char(c.key)))
                    .cloned()
                    .collect();
                help::draw_help_popup(frame, area, theme, &commands);
            }
            if let Some(picker) = theme_picker {
                theme_picker::draw(frame, area, theme, picker, active_theme);
            }
            if let Some(error) = &refresh_error {
                frame.render_widget(
                    Paragraph::new(error.as_str()),
                    Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                );
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
    Ok(ReloadViewport {
        left_viewport: pane_viewport,
        right_viewport: pane_viewport,
        right_width: diff_scrollbar::body_width(sidebar_width::diff_pane_width(sw, area.width)),
    })
}

/// Apply one [`AppCommand`] bubbled up from `apply_events`, in the loop that
/// owns the terminal. Returns `true` when the app should quit.
///
/// Split out of `run()` so the event loop stays readable: custom commands
/// need the terminal, clipboard writes need the mode, and a live
/// theme switch needs the shared [`Theme`] and the shell's force-full flag.
fn apply_app_command(
    cmd: AppCommand,
    terminal: &mut BrowseTerminal,
    mode: &mut Box<dyn Mode>,
    shell: &mut Shell,
    theme: &mut Theme,
) -> bool {
    match cmd {
        AppCommand::Quit => return true,
        // A custom command runs here, in the loop that owns the terminal.
        // Background: run without touching the terminal, so the next draw is
        // unchanged (no flicker). Subprocess: suspend, hand the terminal to
        // the child, restore. Errors are ignored in v1 (the screen is intact
        // / rebuilt regardless).
        AppCommand::Run(run) => {
            if run.subprocess {
                let _ = suspend::run_foreground(terminal, &run.command);
                shell.foreground_returned();
            } else {
                let _ = suspend::run_background(&run.command);
            }
        }
        // Clipboard writes also happen here: the OSC 52 fallback goes to the
        // same stdout the terminal owns. The outcome goes back to the mode so
        // a failed copy is reported honestly.
        AppCommand::CopyToClipboard(text) => {
            let result = clipboard::copy(&text);
            mode.report_copy(result);
        }
        // Apply a live syntax-theme switch. Updating the shared `Theme`
        // changes every diff cache's epoch (which now includes the theme
        // name), so the forced `Full` frame re-highlights every cached block
        // without re-parsing.
        AppCommand::SetSyntaxTheme(name) => {
            theme.syntax_theme_name = name.to_string();
            shell.force_full = true;
            shell.repaint = true;
        }
        AppCommand::Continue => {}
    }
    false
}

/// The shell's owned, mode-agnostic state.
struct Shell {
    /// Sidebar width preference.
    sidebar_pref: Preference,
    /// True while the left button is held on the pane divider.
    dragging_divider: bool,
    /// Whether the help popup is shown.
    help_visible: bool,
    /// Open syntax-theme picker, or `None` when closed.
    theme_picker: Option<ThemePicker>,
    /// Current syntax-theme name (mirrors the shared `Theme`'s
    /// `syntax_theme_name`). Seeds the picker's initial cursor and marks the
    /// active row; the `run()` loop keeps the shared `Theme` in lockstep.
    syntax_theme: String,
    /// Current diff change-layout. Cycled with `\`.
    change_layout: ChangeLayout,
    /// One-shot: force the next frame to draw `Full` (set by a layout
    /// toggle so the diff rebuilds without a scroll-resetting placeholder).
    force_full: bool,
    /// Last-drawn left-column rect, for divider hit-testing.
    left_rect: Rect,
    /// The mode's change receiver, armed after the first build.
    receiver: Option<ChangeReceiver>,
    watch_error: Option<String>,
    watch_retry_at: Option<Instant>,
    watch_retry_delay: Duration,
    reload_retry_at: Option<Instant>,
    refresh_now: bool,
    reload_error: Option<String>,
    /// Whether the mode's watcher has been armed yet.
    armed: bool,
    /// Whether the mode has been built for real yet (vs the startup
    /// empty placeholder).
    built: bool,
    /// Last-known terminal width, for sizing the lazily-built mode.
    total_width: u16,
    /// Dirty timestamp (first change of the current batch).
    dirty_since: Option<Instant>,
    /// Set when the user returns (focus regained, foreground command
    /// finished) to force an immediate reload if the mode is dirty.
    resume_pending: bool,
    /// Whether the most recent input burst was empty (a poll timeout). When
    /// false, the user is actively navigating and the next frame draws
    /// `Fast`; when true, it draws `Full`.
    input_idle: bool,
    /// Unknown focus permits interaction when the terminal sends no reports.
    focused: Option<bool>,
    repaint: bool,
    settle_pending: bool,
    /// User-configured custom key commands (from `config.toml`).
    commands: Vec<CustomCommand>,
}

impl Shell {
    fn new(sidebar_pref: Preference, total_width: u16, syntax_theme: String) -> Self {
        Self {
            sidebar_pref,
            dragging_divider: false,
            help_visible: false,
            theme_picker: None,
            syntax_theme,
            change_layout: ChangeLayout::Grouped,
            force_full: false,
            left_rect: Rect::default(),
            receiver: None,
            armed: false,
            watch_error: None,
            watch_retry_at: None,
            watch_retry_delay: DEBOUNCE_DELAY,
            reload_retry_at: None,
            refresh_now: false,
            reload_error: None,
            built: false,
            total_width,
            dirty_since: None,
            resume_pending: false,
            input_idle: true,
            focused: None,
            repaint: true,
            settle_pending: false,
            commands: Vec::new(),
        }
    }

    fn interactive(&self) -> bool {
        self.focused != Some(false)
    }

    fn needs_redraw(&self) -> bool {
        self.interactive() && self.repaint
    }

    fn poll_background(&mut self, mode: &mut Box<dyn Mode>) -> bool {
        let active = self.interactive() && self.built;
        let work = mode.background(active);
        if mode.take_refresh_request() {
            self.dirty_since.get_or_insert_with(Instant::now);
            self.refresh_now = true;
        }
        if !active {
            return false;
        }
        self.repaint |= work.changed;
        work.pending
    }

    fn resume(&mut self) {
        if !self.interactive() {
            self.resume_pending = true;
            self.force_full = true;
        }
        self.focused = Some(true);
        self.repaint = true;
    }

    fn foreground_returned(&mut self) {
        self.focused = None;
        self.resume_pending = true;
        self.force_full = true;
        self.repaint = true;
    }

    fn refresh_error(&self) -> Option<String> {
        self.watch_error
            .as_ref()
            .map(|err| format!("Auto-refresh unavailable: {err}"))
            .or_else(|| {
                self.reload_error
                    .as_ref()
                    .map(|err| format!("Refresh failed; retrying: {err}"))
            })
    }

    /// Install once, or replace a failed backend with bounded backoff.
    fn arm(&mut self, mode: &mut Box<dyn Mode>) {
        if self.armed {
            return;
        }
        match mode.watch() {
            Ok(receiver) => {
                if self.watch_error.take().is_some() && receiver.is_some() {
                    self.dirty_since.get_or_insert_with(Instant::now);
                }
                self.receiver = receiver;
                self.armed = true;
                self.watch_retry_at = None;
                self.watch_retry_delay = DEBOUNCE_DELAY;
            }
            Err(err) => self.watch_failed(err),
        }
    }

    fn watch_failed(&mut self, error: String) {
        self.receiver = None;
        self.armed = false;
        self.watch_error = Some(error);
        self.watch_retry_at = Some(Instant::now() + self.watch_retry_delay);
        self.watch_retry_delay = (self.watch_retry_delay * 2).min(MAX_WATCH_RETRY_DELAY);
    }

    /// Take one bounded batch from the watcher. Events arriving during a
    /// reload remain in the accumulator until the next loop.
    fn drain_watchers(&mut self, mode: &mut Box<dyn Mode>) {
        if !self.interactive() {
            return;
        }
        let previous_error = self.refresh_error();
        self.drain_watcher(mode);
        self.repaint |= self.refresh_error() != previous_error;
    }

    fn drain_watcher(&mut self, mode: &mut Box<dyn Mode>) {
        if self.built && !self.armed && self.watch_retry_at.is_none_or(|at| Instant::now() >= at) {
            self.arm(mode);
        }
        let Some(receiver) = self.receiver.as_ref() else {
            return;
        };
        let batch = receiver.take();
        if let Some(error) = batch.error {
            self.watch_failed(error);
            return;
        }
        let paths: Vec<_> = batch.paths.into_iter().collect();
        if (batch.rescan || !paths.is_empty()) && mode.notify_changes(&paths, batch.rescan) {
            self.dirty_since.get_or_insert_with(Instant::now);
        }
    }

    /// The draw budget for the current frame: `Full` when input has
    /// settled, `Fast` while a navigation key is streaming so the mode can
    /// defer expensive rendering.
    ///
    /// A one-shot `force_full` overrides the streaming heuristic for the
    /// next frame and is consumed here. A layout toggle sets it so the diff
    /// rebuilds fully at once instead of flashing `Fast` placeholders (whose
    /// short window would clamp the saved scroll to the top).
    fn take_draw_budget(&mut self) -> DrawBudget {
        self.repaint = false;
        if std::mem::take(&mut self.force_full) || self.input_idle {
            self.settle_pending = false;
            DrawBudget::Full
        } else {
            self.settle_pending = true;
            DrawBudget::Fast
        }
    }

    /// Record whether the just-read input burst was empty (a poll timeout,
    /// i.e. input settled). Drives the next frame's [`Shell::draw_budget`].
    fn note_input(&mut self, burst_empty: bool) {
        self.input_idle = burst_empty;
        if burst_empty && self.settle_pending {
            self.repaint = true;
        }
    }

    /// Pick the loop's poll timeout: short while a reload is pending,
    /// otherwise the idle timeout.
    fn poll_timeout(&self) -> Duration {
        if !self.interactive() {
            return POLL_TIMEOUT;
        }
        if self.refresh_now {
            return Duration::ZERO;
        }
        let mut timeout = if self.input_idle {
            POLL_TIMEOUT
        } else {
            SETTLE_TIMEOUT
        };
        if let Some(since) = self.dirty_since {
            timeout = timeout.min(DEBOUNCE_DELAY.saturating_sub(since.elapsed()));
        }
        if let Some(at) = self.reload_retry_at {
            timeout = timeout.min(at.saturating_duration_since(Instant::now()));
        }
        if let Some(at) = self.watch_retry_at {
            timeout = timeout.min(at.saturating_duration_since(Instant::now()));
        }
        timeout
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

    /// Build the mode for real once its loading frame is on screen.
    /// No-op if already built. The mode watches before loading its snapshot;
    /// retain those pending events and schedule recovery if the read failed.
    fn build(&mut self, mode: &mut Box<dyn Mode>, vp: ReloadViewport, theme: &Theme) {
        if self.built || !self.interactive() {
            return;
        }
        let dw = if vp.right_width > 0 {
            vp.right_width
        } else {
            diff_scrollbar::body_width(sidebar_width::diff_pane_width(
                self.sidebar_pref.effective(self.total_width),
                self.total_width,
            ))
        };
        *mode = Box::new(FilesMode::build(theme, dw));
        self.built = true;
        self.dirty_since = None;
        self.arm(mode);
        if mode.retry_reload() {
            self.reload_retry_at = Some(Instant::now() + DEBOUNCE_DELAY);
        }
        self.resume_pending = false;
        self.repaint = true;
    }

    /// Reload the mode if its debounce has elapsed, or right away when the
    /// user just returned while it was dirty.
    fn reload_if_due(
        &mut self,
        mode: &mut Box<dyn Mode>,
        vp: ReloadViewport,
        theme: &Theme,
    ) -> Result<(), String> {
        if !self.interactive() {
            return Ok(());
        }
        let due = self.refresh_now
            || self
                .dirty_since
                .is_some_and(|s| s.elapsed() >= DEBOUNCE_DELAY);
        let retry_due = self.reload_retry_at.is_some_and(|at| Instant::now() >= at);
        if due || retry_due || self.resume_pending {
            if self.dirty_since.is_some() || self.reload_retry_at.is_some() {
                let previous_error = self.refresh_error();
                let result = mode.reload(vp, theme);
                let changed = result.as_ref().is_ok_and(|changed| *changed);
                self.dirty_since = None;
                self.refresh_now = false;
                let failed = result.is_err() || mode.retry_reload();
                self.reload_error = result.err();
                self.reload_retry_at = failed.then(|| Instant::now() + READ_RETRY_DELAY);
                self.repaint |= changed || self.refresh_error() != previous_error;
                self.force_full |= changed;
            }
            self.resume_pending = false;
        }
        Ok(())
    }

    /// Handle a key already in the shell. Global bindings are consumed
    /// here; everything else routes to the mode.
    fn handle_key(
        &mut self,
        mode: &mut Box<dyn Mode>,
        key: KeyCode,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        // A mode with an open text editor owns every key, so global
        // bindings and custom-command keys are typed as literal text —
        // `q` included — and `Esc` closes the editor. Checked first, since
        // there is no way to type a `q` otherwise.
        if mode.captures_text_input() {
            return mode.handle_key(key, left_viewport, right_viewport);
        }
        // `q` quits from anywhere, popup or not.
        if key == KeyCode::Char('q') {
            return AppCommand::Quit;
        }
        if self.help_visible {
            return help::handle_key_help(&mut self.help_visible, key);
        }
        // The theme picker is modal: while open it owns every key (except
        // the `q` quit handled above), so navigation never leaks to the mode.
        if self.theme_picker.is_some() {
            return self.handle_theme_picker_key(key);
        }
        match key {
            KeyCode::Char('?') => {
                self.help_visible = true;
                AppCommand::Continue
            }
            // `t` opens the syntax-theme picker, seeded at the current theme.
            KeyCode::Char('t') => {
                self.theme_picker = Some(ThemePicker::open(&self.syntax_theme));
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
            // `\` cycles the diff change-layout. The cache
            // keys on layout, so the next draw rebuilds.
            KeyCode::Char('\\') => {
                self.change_layout = next_layout(self.change_layout);
                // Rebuild fully next frame so the diff keeps its scroll
                // position instead of snapping to the top.
                self.force_full = true;
                AppCommand::Continue
            }
            key if mode.reserves_key(key) => mode.handle_key(key, left_viewport, right_viewport),
            // Custom commands take priority over unreserved mode keys (but not
            // over the shell globals above). A bound key with a selectable
            // file expands and bubbles up a Run request; with nothing
            // selected it is a silent no-op.
            KeyCode::Char(c) if self.command_for(c).is_some() => {
                let cmd = self.command_for(c).expect("checked by guard");
                match mode.selected_path() {
                    Some(path) => AppCommand::Run(CustomRun {
                        command: command::expand(&cmd.command, &path),
                        subprocess: cmd.subprocess,
                    }),
                    None => AppCommand::Continue,
                }
            }
            other => mode.handle_key(other, left_viewport, right_viewport),
        }
    }

    /// Drive the open theme picker with one key. Picking bubbles an
    /// [`AppCommand::SetSyntaxTheme`] to the `run()` loop (which owns the
    /// shared `Theme`) and mirrors the choice onto the shell so the next
    /// open highlights it.
    fn handle_theme_picker_key(&mut self, key: KeyCode) -> AppCommand {
        let Some(picker) = self.theme_picker.as_mut() else {
            return AppCommand::Continue;
        };
        match picker.handle_key(key) {
            PickerAction::Stay => AppCommand::Continue,
            PickerAction::Close => {
                self.theme_picker = None;
                AppCommand::Continue
            }
            PickerAction::Pick(name) => {
                self.syntax_theme = name.to_string();
                self.theme_picker = None;
                AppCommand::SetSyntaxTheme(name)
            }
        }
    }

    /// The custom command bound to `key`, if any.
    fn command_for(&self, key: char) -> Option<&CustomCommand> {
        self.commands.iter().find(|c| c.key == key)
    }

    /// Handle a mouse event. Divider drag is resolved here; everything
    /// else routes to the mode.
    fn handle_mouse(
        &mut self,
        mode: &mut Box<dyn Mode>,
        mouse: MouseEvent,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        if self.help_visible || self.theme_picker.is_some() {
            return AppCommand::Continue;
        }
        if mode.captures_text_input() {
            return mode.handle_mouse(mouse, left_viewport, right_viewport);
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
        mode.handle_mouse(mouse, left_viewport, right_viewport)
    }

    /// Apply a whole burst of input events, stopping early on `Quit`.
    /// Sidebar-resize keys (`<`/`>`) are coalesced to one step per burst.
    fn apply_events(
        &mut self,
        mode: &mut Box<dyn Mode>,
        events: impl IntoIterator<Item = Event>,
        vp: ReloadViewport,
        _theme: &Theme,
    ) -> Result<AppCommand, String> {
        let mut resized = false;
        let mut command = AppCommand::Continue;
        for event in events {
            if command != AppCommand::Continue
                && !matches!(
                    event,
                    Event::FocusLost | Event::FocusGained | Event::Resize(_, _)
                )
            {
                continue;
            }
            // Coalesce sidebar-resize keys to one step per burst.
            if is_resize_key(&event) && std::mem::replace(&mut resized, true) {
                continue;
            }
            let cmd = self.dispatch(mode, event, vp);
            if cmd == AppCommand::Quit {
                return Ok(cmd);
            }
            if command == AppCommand::Continue {
                command = cmd;
            }
        }
        Ok(command)
    }

    /// Dispatch one input event to the global handlers / the mode.
    fn dispatch(
        &mut self,
        mode: &mut Box<dyn Mode>,
        event: Event,
        vp: ReloadViewport,
    ) -> AppCommand {
        match event {
            Event::FocusLost => {
                self.focused = Some(false);
                AppCommand::Continue
            }
            Event::FocusGained => {
                self.resume();
                self.force_full = true;
                AppCommand::Continue
            }
            Event::Resize(_, _) => {
                self.repaint = true;
                self.force_full = true;
                AppCommand::Continue
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.resume();
                self.handle_key(mode, key.code, vp.left_viewport, vp.right_viewport)
            }
            Event::Mouse(mouse) => {
                self.resume();
                self.handle_mouse(mode, mouse, vp.left_viewport, vp.right_viewport)
            }
            _ => AppCommand::Continue,
        }
    }
}

/// Render the loading frame shown before the mode is built: the sidebar
/// title plus a centered `Loading…` message in both columns.
fn draw_loading(frame: &mut ratatui::Frame<'_>, left: Rect, right: Rect, theme: &Theme) {
    let border = rgb_to_color(theme.border);
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let loading = |block| {
        Paragraph::new("Loading…")
            .style(muted)
            .alignment(Alignment::Center)
            .block(block)
    };
    frame.render_widget(
        loading(pane_block_with_title_line(
            files::sidebar_title(border, theme),
            border,
            None,
        )),
        left,
    );
    frame.render_widget(loading(pane_block("─Diff─", border)), right);
}

/// Toggle the diff change-layout: `Grouped ⇆ interleaved`. Interleaved uses
/// a group size of 1 (each removed line next to its replacement).
fn next_layout(layout: ChangeLayout) -> ChangeLayout {
    use std::num::NonZeroUsize;
    match layout {
        ChangeLayout::Grouped => ChangeLayout::Interleaved {
            group: NonZeroUsize::new(1).expect("1 is non-zero"),
        },
        ChangeLayout::Interleaved { .. } => ChangeLayout::Grouped,
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
