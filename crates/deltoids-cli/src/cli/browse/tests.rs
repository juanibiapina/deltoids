//! Shell tests: global key handling, routing to the mode, divider drag,
//! sidebar resize, and reload timing. Driven against the [`Mode`]
//! interface via a recording mock, so they describe shell behaviour
//! independent of `FilesMode`.

use crate::cli::browse::watch::ChangeReceiver;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use deltoids::Theme;

use super::*;

#[derive(Default)]
struct Recorder {
    keys: Vec<KeyCode>,
    mouse: usize,
    reloads: usize,
    read_failures: usize,
    stable_reads: usize,
    watch_failures: usize,
    reserve_actions: bool,
    refresh_requested: bool,
}

struct RecordingMode {
    rec: Rc<RefCell<Recorder>>,
    selected: Option<PathBuf>,
    /// Stands in for a mode with an open text editor.
    capturing: bool,
}

impl RecordingMode {
    fn new() -> (Self, Rc<RefCell<Recorder>>) {
        let rec = Rc::new(RefCell::new(Recorder::default()));
        (
            Self {
                rec: rec.clone(),
                selected: None,
                capturing: false,
            },
            rec,
        )
    }
}

impl Mode for RecordingMode {
    fn draw(
        &mut self,
        _frame: &mut ratatui::Frame<'_>,
        _left: Rect,
        _right: Rect,
        _layout: deltoids::ChangeLayout,
        _theme: &Theme,
        _budget: DrawBudget,
    ) {
    }

    fn handle_key(&mut self, key: KeyCode, _lv: usize, _rv: usize) -> AppCommand {
        self.rec.borrow_mut().keys.push(key);
        AppCommand::Continue
    }

    fn captures_text_input(&self) -> bool {
        self.capturing
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.rec.borrow().reserve_actions && matches!(key, KeyCode::Char(' ' | 'd' | 'a'))
    }

    fn take_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.rec.borrow_mut().refresh_requested)
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _lv: usize, _rv: usize) -> AppCommand {
        self.rec.borrow_mut().mouse += 1;
        AppCommand::Continue
    }

    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String> {
        let mut rec = self.rec.borrow_mut();
        if rec.watch_failures > 0 {
            rec.watch_failures -= 1;
            return Err("watch installation failed".into());
        }
        Ok(None)
    }

    fn should_reload(&self, _paths: &[PathBuf]) -> bool {
        true
    }

    fn retry_reload(&self) -> bool {
        false
    }

    fn reload(&mut self, _viewport: ReloadViewport, _theme: &Theme) -> Result<bool, String> {
        let mut rec = self.rec.borrow_mut();
        rec.reloads += 1;
        if rec.read_failures > 0 {
            rec.read_failures -= 1;
            return Err("read lost a race".into());
        }
        if rec.stable_reads > 0 {
            rec.stable_reads -= 1;
            return Ok(false);
        }
        Ok(true)
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected.clone()
    }
}

#[path = "tests/focus.rs"]
mod focus;

type Rec = Rc<RefCell<Recorder>>;

fn one_mode() -> (Box<dyn Mode>, Rec) {
    let (mode, rec) = RecordingMode::new();
    (Box::new(mode), rec)
}

fn shell() -> Shell {
    let mut s = Shell::new(Preference::seeded(200), 200, "TokyoNight".to_string());
    // The mock mode is already real; mark it built so the shell never
    // replaces it with a concrete FilesMode.
    s.built = true;
    s
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn completed_action_refreshes_the_active_mode_immediately() {
    let (mut mode, files) = one_mode();
    let mut s = shell();
    files.borrow_mut().refresh_requested = true;
    let start = Instant::now();
    s.poll_background(&mut mode);
    s.reload_if_due(&mut mode, ReloadViewport::default(), &Theme::default())
        .unwrap();
    assert_eq!(
        files.borrow().reloads,
        1,
        "completed action was not refreshed after {:?}; scheduled timeout={:?}",
        start.elapsed(),
        s.poll_timeout()
    );
}

#[test]
fn reserved_mode_actions_beat_custom_commands() {
    let (mut mode, files) = one_mode();
    let mut s = shell();
    s.commands = vec![
        custom_command('a', "touch unwanted", false),
        custom_command('d', "touch unwanted", false),
    ];
    files.borrow_mut().reserve_actions = true;
    for key in ['a', 'd'] {
        assert_eq!(
            s.handle_key(&mut mode, KeyCode::Char(key), 20, 20),
            AppCommand::Continue
        );
    }
    assert_eq!(
        files.borrow().keys,
        vec![KeyCode::Char('a'), KeyCode::Char('d')]
    );
}

#[test]
fn modal_input_blocks_divider_drags() {
    let mut s = shell();
    let (mut capturing, _) = RecordingMode::new();
    capturing.capturing = true;
    let mut mode: Box<dyn Mode> = Box::new(capturing);
    s.left_rect = Rect::new(0, 0, 50, 20);
    s.handle_mouse(
        &mut mode,
        mouse(MouseEventKind::Down(MouseButton::Left), 49, 3),
        20,
        20,
    );
    assert!(!s.dragging_divider);
}

#[test]
fn q_quits_and_esc_routes_to_the_active_mode() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    assert_eq!(
        s.handle_key(&mut mode, KeyCode::Char('q'), 4, 4),
        AppCommand::Quit
    );
    // Esc is not a quit key outside the help popup: it reaches the mode.
    assert_eq!(
        s.handle_key(&mut mode, KeyCode::Esc, 4, 4),
        AppCommand::Continue
    );
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Esc]);
}

#[test]
fn draw_budget_follows_input_idle() {
    let mut s = shell();
    // Seeded idle: the first frame draws in full.
    assert_eq!(s.take_draw_budget(), DrawBudget::Full);

    // A non-empty burst (active navigation) makes the next frame Fast and
    // shortens the poll so the settled frame lands quickly.
    s.note_input(false);
    assert_eq!(s.take_draw_budget(), DrawBudget::Fast);
    assert_eq!(s.poll_timeout(), SETTLE_TIMEOUT);

    // An empty burst (settled) returns to Full and the idle poll timeout.
    s.note_input(true);
    assert_eq!(s.take_draw_budget(), DrawBudget::Full);
    assert_eq!(s.poll_timeout(), POLL_TIMEOUT);
}

#[test]
fn question_mark_toggles_help_and_help_swallows_keys() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    s.handle_key(&mut mode, KeyCode::Char('?'), 4, 4);
    assert!(s.help_visible);

    // While help is up, Esc closes the popup and does not reach the
    // active mode.
    let cmd = s.handle_key(&mut mode, KeyCode::Esc, 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    assert!(!s.help_visible);
    assert!(files_rec.borrow().keys.is_empty());
}

#[test]
fn t_opens_theme_picker_and_picking_applies_and_closes() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    // `t` opens the modal picker; the key does not reach the active mode.
    s.handle_key(&mut mode, KeyCode::Char('t'), 4, 4);
    assert!(s.theme_picker.is_some());
    assert!(files_rec.borrow().keys.is_empty());

    // Navigation is swallowed by the modal picker.
    let cmd = s.handle_key(&mut mode, KeyCode::Char('j'), 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    assert!(files_rec.borrow().keys.is_empty());

    // Enter applies the cursor theme, bubbles SetSyntaxTheme, and closes.
    let cmd = s.handle_key(&mut mode, KeyCode::Enter, 4, 4);
    match cmd {
        AppCommand::SetSyntaxTheme(name) => {
            assert_eq!(name, s.syntax_theme);
        }
        other => panic!("expected SetSyntaxTheme, got {other:?}"),
    }
    assert!(s.theme_picker.is_none());
}

#[test]
fn esc_closes_theme_picker_without_applying() {
    let (mut mode, _) = one_mode();
    let mut s = shell();
    s.handle_key(&mut mode, KeyCode::Char('t'), 4, 4);
    assert!(s.theme_picker.is_some());
    let before = s.syntax_theme.clone();
    let cmd = s.handle_key(&mut mode, KeyCode::Esc, 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    assert!(s.theme_picker.is_none());
    assert_eq!(s.syntax_theme, before, "Esc must not change the theme");
}

#[test]
fn q_quits_even_with_the_help_popup_open() {
    let (mut mode, _) = one_mode();
    let mut s = shell();
    s.handle_key(&mut mode, KeyCode::Char('?'), 4, 4);
    assert!(s.help_visible);
    assert_eq!(
        s.handle_key(&mut mode, KeyCode::Char('q'), 4, 4),
        AppCommand::Quit
    );
}

#[test]
fn brackets_route_to_the_mode() {
    let (mut mode, rec) = one_mode();
    let mut s = shell();
    s.handle_key(&mut mode, KeyCode::Char(']'), 4, 4);
    s.handle_key(&mut mode, KeyCode::Char('['), 4, 4);
    assert_eq!(
        rec.borrow().keys,
        vec![KeyCode::Char(']'), KeyCode::Char('[')]
    );
}

#[test]
fn resize_keys_change_sidebar_width() {
    let (mut mode, _) = one_mode();
    let mut s = shell();
    let initial = s.sidebar_pref.effective(200);
    s.handle_key(&mut mode, KeyCode::Char('>'), 4, 4);
    assert!(s.sidebar_pref.effective(200) > initial);
    s.handle_key(&mut mode, KeyCode::Char('<'), 4, 4);
    assert_eq!(s.sidebar_pref.effective(200), initial);
}

#[test]
fn backslash_toggles_grouped_and_interleaved() {
    use deltoids::ChangeLayout;
    use std::num::NonZeroUsize;
    let interleaved = ChangeLayout::Interleaved {
        group: NonZeroUsize::new(1).unwrap(),
    };
    let (mut mode, _) = one_mode();
    let mut s = shell();
    // Starts grouped, toggles to interleaved, and back.
    assert_eq!(s.change_layout, ChangeLayout::Grouped);
    for expected in [interleaved, ChangeLayout::Grouped] {
        s.handle_key(&mut mode, KeyCode::Char('\\'), 4, 4);
        assert_eq!(s.change_layout, expected);
    }
}

#[test]
fn backslash_forces_a_full_frame_so_scroll_survives() {
    // A layout toggle clears the diff cache; the next frame must be Full
    // (not a Fast placeholder that would clamp the saved scroll to the top).
    let (mut mode, _) = one_mode();
    let mut s = shell();
    // Simulate a streaming frame: input just arrived, so the budget would
    // otherwise be Fast.
    s.note_input(false);
    s.handle_key(&mut mode, KeyCode::Char('\\'), 4, 4);
    assert_eq!(s.take_draw_budget(), DrawBudget::Full);
    // The flag is one-shot: the following frame falls back to the streaming
    // heuristic (still Fast until input settles).
    assert_eq!(s.take_draw_budget(), DrawBudget::Fast);
}

#[test]
fn divider_drag_resizes_and_release_ends() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    s.left_rect = Rect::new(0, 0, 38, 20); // divider at cols 37 / 38
    assert!(s.is_on_divider(37));
    assert!(s.is_on_divider(38));
    assert!(!s.is_on_divider(5));

    s.handle_mouse(
        &mut mode,
        mouse(MouseEventKind::Down(MouseButton::Left), 37, 5),
        18,
        18,
    );
    assert!(s.dragging_divider);
    // The mode never saw the divider press.
    assert_eq!(files_rec.borrow().mouse, 0);

    s.handle_mouse(
        &mut mode,
        mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5),
        18,
        18,
    );
    assert_eq!(s.sidebar_pref.effective(200), 51);

    s.handle_mouse(
        &mut mode,
        mouse(MouseEventKind::Up(MouseButton::Left), 50, 5),
        18,
        18,
    );
    assert!(!s.dragging_divider);
}

#[test]
fn non_divider_mouse_routes_to_active_mode() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    s.left_rect = Rect::new(0, 0, 38, 20);
    s.handle_mouse(&mut mode, mouse(MouseEventKind::ScrollDown, 50, 5), 18, 18);
    assert_eq!(files_rec.borrow().mouse, 1);
}

#[test]
fn apply_events_coalesces_repeated_resize_keys() {
    let (mut mode, _) = one_mode();
    let mut s = shell();
    let vp = ReloadViewport::default();
    let theme = Theme::default();
    let initial = s.sidebar_pref.effective(200);
    let burst = vec![
        Event::Key(key(KeyCode::Char('>'))),
        Event::Key(key(KeyCode::Char('>'))),
        Event::Key(key(KeyCode::Char('>'))),
        Event::Key(key(KeyCode::Char('>'))),
    ];
    s.apply_events(&mut mode, burst, vp, &theme).unwrap();
    // One step per burst, not one per repeat.
    assert_eq!(s.sidebar_pref.effective(200), initial + 4);
}

#[test]
fn loading_frame_shows_sidebar_title_and_message() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::{Constraint, Direction, Layout};

    let theme = Theme::default();
    let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
    term.draw(|f| {
        let area = f.area();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(10)])
            .split(area);
        draw_loading(f, cols[0], cols[1], &theme);
    })
    .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("Loading"),
        "loading message missing: {text:?}"
    );
    assert!(text.contains("Files"), "sidebar title missing: {text:?}");
}

fn custom_command(key: char, command: &str, subprocess: bool) -> CustomCommand {
    CustomCommand {
        key,
        command: command.to_string(),
        subprocess,
        description: String::new(),
    }
}

#[test]
fn custom_key_with_selected_path_returns_run() {
    let (_, files_rec) = RecordingMode::new();
    let mut mode: Box<dyn Mode> = Box::new(RecordingMode {
        rec: files_rec.clone(),
        selected: Some(PathBuf::from("/tmp/a.txt")),
        capturing: false,
    });
    let mut s = shell();
    s.commands = vec![custom_command('e', "nvim {{filename}}", false)];

    let cmd = s.handle_key(&mut mode, KeyCode::Char('e'), 4, 4);
    assert_eq!(
        cmd,
        AppCommand::Run(CustomRun {
            command: "nvim '/tmp/a.txt'".to_string(),
            subprocess: false,
        })
    );
    // The key never reached the mode's own handler.
    assert!(files_rec.borrow().keys.is_empty());
}

#[test]
fn custom_key_preserves_subprocess_flag() {
    let (_, files_rec) = RecordingMode::new();
    let mut mode: Box<dyn Mode> = Box::new(RecordingMode {
        rec: files_rec,
        selected: Some(PathBuf::from("/tmp/a.txt")),
        capturing: false,
    });
    let mut s = shell();
    s.commands = vec![custom_command('E', "nvim {{filename}}", true)];

    let cmd = s.handle_key(&mut mode, KeyCode::Char('E'), 4, 4);
    assert_eq!(
        cmd,
        AppCommand::Run(CustomRun {
            command: "nvim '/tmp/a.txt'".to_string(),
            subprocess: true,
        })
    );
}

#[test]
fn custom_key_with_no_selection_is_noop() {
    // The default mock mode has no selected path.
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    s.commands = vec![custom_command('e', "nvim {{filename}}", false)];

    let cmd = s.handle_key(&mut mode, KeyCode::Char('e'), 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    // Silent no-op: the key is consumed, not routed to the mode.
    assert!(files_rec.borrow().keys.is_empty());
}

#[test]
fn unconfigured_key_routes_to_mode() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    s.commands = vec![custom_command('e', "nvim {{filename}}", false)];
    s.handle_key(&mut mode, KeyCode::Char('x'), 4, 4);
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Char('x')]);
}

#[test]
fn global_builtins_beat_colliding_custom_binding() {
    let (_, files_rec) = RecordingMode::new();
    let mut mode: Box<dyn Mode> = Box::new(RecordingMode {
        rec: files_rec,
        selected: Some(PathBuf::from("/tmp/a.txt")),
        capturing: false,
    });
    let mut s = shell();
    // Bind the same keys as the `q` quit and `>` resize globals.
    s.commands = vec![
        custom_command('q', "echo q", false),
        custom_command('>', "echo widen", false),
    ];
    // `q` still quits.
    assert_eq!(
        s.handle_key(&mut mode, KeyCode::Char('q'), 4, 4),
        AppCommand::Quit
    );
    // `>` still widens the sidebar, does not run the custom command.
    let initial = s.sidebar_pref.effective(200);
    let cmd = s.handle_key(&mut mode, KeyCode::Char('>'), 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    assert!(s.sidebar_pref.effective(200) > initial);
}

#[test]
fn apply_events_quit_short_circuits() {
    let (mut mode, files_rec) = one_mode();
    let mut s = shell();
    let vp = ReloadViewport::default();
    let theme = Theme::default();
    let burst = vec![
        Event::Key(key(KeyCode::Char('j'))),
        Event::Key(key(KeyCode::Char('q'))),
        Event::Key(key(KeyCode::Char('j'))),
    ];
    assert_eq!(
        s.apply_events(&mut mode, burst, vp, &theme).unwrap(),
        AppCommand::Quit
    );
    // Only the first j reached the mode.
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Char('j')]);
}

#[test]
fn capturing_mode_receives_every_key_including_globals() {
    let (_, files_rec) = RecordingMode::new();
    let mut mode: Box<dyn Mode> = Box::new(RecordingMode {
        rec: files_rec.clone(),
        selected: Some(PathBuf::from("/tmp/a.txt")),
        capturing: true,
    });
    let mut s = shell();
    // A custom binding that would otherwise shadow the mode's own keys.
    s.commands = vec![custom_command('e', "nvim {{filename}}", false)];

    for code in [
        KeyCode::Char('q'),
        KeyCode::Char('?'),
        KeyCode::Char('<'),
        KeyCode::Char('>'),
        KeyCode::Char('e'),
        KeyCode::Esc,
    ] {
        assert_eq!(
            s.handle_key(&mut mode, code, 4, 4),
            AppCommand::Continue,
            "{code:?} must not act as a global while a mode captures input"
        );
    }

    assert_eq!(files_rec.borrow().keys.len(), 6);
    assert!(!s.help_visible, "the help popup never opened");
}

#[test]
fn idle_mode_does_not_reload_and_rescan_work_is_consumed_once() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.armed = true;
    let theme = Theme::default();
    let viewport = ReloadViewport::default();
    for _ in 0..1000 {
        shell.drain_watchers(&mut mode);
        shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    }
    assert_eq!(files.borrow().reloads, 0);

    let dir = tempfile::tempdir().unwrap();
    let watcher = watch::ChangeWatcher::new(&[dir.path()]).unwrap();
    let receiver = watcher.receiver();
    receiver.request_rescan();
    shell.receiver = Some(receiver);
    shell.drain_watchers(&mut mode);
    shell.dirty_since = Some(Instant::now() - DEBOUNCE_DELAY);
    shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    shell.drain_watchers(&mut mode);
    shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    assert_eq!(files.borrow().reloads, 1);
}

#[test]
fn transient_read_retries_without_another_event_and_stops_after_success() {
    let (mut mode, files) = one_mode();
    files.borrow_mut().read_failures = 1;
    let mut shell = shell();
    shell.dirty_since = Some(Instant::now() - DEBOUNCE_DELAY);
    let theme = Theme::default();
    let viewport = ReloadViewport::default();
    shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    assert!(shell.reload_error.is_some());
    shell.reload_retry_at = Some(Instant::now());
    shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    assert!(shell.reload_error.is_none());
    assert!(shell.reload_retry_at.is_none());
    shell.reload_if_due(&mut mode, viewport, &theme).unwrap();
    assert_eq!(files.borrow().reloads, 2);
}

#[test]
fn watcher_failure_is_visible_and_installation_retries_without_scanning() {
    let (mut mode, files) = one_mode();
    files.borrow_mut().watch_failures = 1;
    let mut shell = shell();
    shell.arm(&mut mode);
    assert!(shell.watch_error.is_some());
    assert!(!shell.poll_timeout().is_zero());
    shell.watch_retry_at = Some(Instant::now());
    shell.drain_watchers(&mut mode);
    assert!(shell.watch_error.is_none());
    assert!(shell.armed);
    assert_eq!(files.borrow().reloads, 0);
}
