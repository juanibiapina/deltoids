//! Shell tests: global key handling, mode toggle, routing to the active
//! mode, divider drag, sidebar resize, and lazy reload on toggle. Driven
//! against the [`Mode`] interface via a recording mock, so they describe
//! shell behaviour independent of either concrete mode.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use deltoids::Theme;

use super::*;

#[derive(Default)]
struct Recorder {
    keys: Vec<KeyCode>,
    mouse: usize,
    reloads: usize,
}

struct RecordingMode {
    rec: Rc<RefCell<Recorder>>,
}

impl RecordingMode {
    fn new() -> (Self, Rc<RefCell<Recorder>>) {
        let rec = Rc::new(RefCell::new(Recorder::default()));
        (Self { rec: rec.clone() }, rec)
    }
}

impl Mode for RecordingMode {
    fn draw(
        &mut self,
        _frame: &mut ratatui::Frame<'_>,
        _left: Rect,
        _right: Rect,
        _tabs: TabStrip,
        _theme: &Theme,
    ) {
    }

    fn handle_key(&mut self, key: KeyCode, _lv: usize, _rv: usize) -> AppCommand {
        self.rec.borrow_mut().keys.push(key);
        AppCommand::Continue
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _lv: usize, _rv: usize) -> AppCommand {
        self.rec.borrow_mut().mouse += 1;
        AppCommand::Continue
    }

    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>> {
        None
    }

    fn should_reload(&self, _paths: &[PathBuf]) -> bool {
        true
    }

    fn needs_git_poll(&self) -> bool {
        false
    }

    fn reload(&mut self, _viewport: ReloadViewport, _theme: &Theme) -> Result<bool, String> {
        self.rec.borrow_mut().reloads += 1;
        Ok(true)
    }
}

type Rec = Rc<RefCell<Recorder>>;

fn three_modes() -> ([Box<dyn Mode>; MODE_COUNT], Rec, Rec, Rec) {
    let (files, files_rec) = RecordingMode::new();
    let (traces, traces_rec) = RecordingMode::new();
    let (live, live_rec) = RecordingMode::new();
    let modes: [Box<dyn Mode>; MODE_COUNT] = [Box::new(files), Box::new(traces), Box::new(live)];
    (modes, files_rec, traces_rec, live_rec)
}

fn shell() -> Shell {
    let mut s = Shell::new(FILES_MODE, Preference::seeded(200), 200);
    // Mock modes are already real; mark them built so a cycle never
    // replaces them with a concrete FilesMode/TracesMode/LiveMode.
    s.built = [true, true, true];
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
fn q_and_esc_quit() {
    let (mut modes, _, _, _) = three_modes();
    let mut s = shell();
    assert_eq!(
        s.handle_key(&mut modes, KeyCode::Char('q'), 4, 4),
        AppCommand::Quit
    );
    assert_eq!(
        s.handle_key(&mut modes, KeyCode::Esc, 4, 4),
        AppCommand::Quit
    );
}

#[test]
fn question_mark_toggles_help_and_help_swallows_keys() {
    let (mut modes, files_rec, _, _) = three_modes();
    let mut s = shell();
    s.handle_key(&mut modes, KeyCode::Char('?'), 4, 4);
    assert!(s.help_visible);

    // While help is up, a quit key only closes the popup and does not
    // reach the active mode.
    let cmd = s.handle_key(&mut modes, KeyCode::Char('q'), 4, 4);
    assert_eq!(cmd, AppCommand::Continue);
    assert!(!s.help_visible);
    assert!(files_rec.borrow().keys.is_empty());
}

#[test]
fn bracket_cycles_active_mode() {
    let (mut modes, _, _, _) = three_modes();
    let mut s = shell();
    assert_eq!(s.active, FILES_MODE);
    // `]` cycles Files -> Traces -> Live -> Files.
    s.handle_key(&mut modes, KeyCode::Char(']'), 4, 4);
    assert_eq!(s.active, TRACES_MODE);
    s.handle_key(&mut modes, KeyCode::Char(']'), 4, 4);
    assert_eq!(s.active, LIVE_MODE);
    s.handle_key(&mut modes, KeyCode::Char(']'), 4, 4);
    assert_eq!(s.active, FILES_MODE);
    // `[` cycles the other way: Files -> Live -> Traces -> Files.
    s.handle_key(&mut modes, KeyCode::Char('['), 4, 4);
    assert_eq!(s.active, LIVE_MODE);
    s.handle_key(&mut modes, KeyCode::Char('['), 4, 4);
    assert_eq!(s.active, TRACES_MODE);
    s.handle_key(&mut modes, KeyCode::Char('['), 4, 4);
    assert_eq!(s.active, FILES_MODE);
}

#[test]
fn nav_keys_route_to_active_mode_only() {
    let (mut modes, files_rec, traces_rec, _) = three_modes();
    let mut s = shell();
    s.handle_key(&mut modes, KeyCode::Char('j'), 4, 4);
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Char('j')]);
    assert!(traces_rec.borrow().keys.is_empty());

    s.cycle(true);
    s.handle_key(&mut modes, KeyCode::Char('k'), 4, 4);
    assert_eq!(traces_rec.borrow().keys, vec![KeyCode::Char('k')]);
    // Files mode never saw the second key.
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Char('j')]);
}

#[test]
fn resize_keys_change_shared_sidebar_width() {
    let (mut modes, _, _, _) = three_modes();
    let mut s = shell();
    let initial = s.sidebar_pref.effective(200);
    s.handle_key(&mut modes, KeyCode::Char('>'), 4, 4);
    assert!(s.sidebar_pref.effective(200) > initial);
    s.handle_key(&mut modes, KeyCode::Char('<'), 4, 4);
    assert_eq!(s.sidebar_pref.effective(200), initial);
}

#[test]
fn divider_drag_resizes_and_release_ends() {
    let (mut modes, files_rec, _, _) = three_modes();
    let mut s = shell();
    s.left_rect = Rect::new(0, 0, 38, 20); // divider at cols 37 / 38
    assert!(s.is_on_divider(37));
    assert!(s.is_on_divider(38));
    assert!(!s.is_on_divider(5));

    s.handle_mouse(
        &mut modes,
        mouse(MouseEventKind::Down(MouseButton::Left), 37, 5),
        18,
        18,
    );
    assert!(s.dragging_divider);
    // The mode never saw the divider press.
    assert_eq!(files_rec.borrow().mouse, 0);

    s.handle_mouse(
        &mut modes,
        mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5),
        18,
        18,
    );
    assert_eq!(s.sidebar_pref.effective(200), 51);

    s.handle_mouse(
        &mut modes,
        mouse(MouseEventKind::Up(MouseButton::Left), 50, 5),
        18,
        18,
    );
    assert!(!s.dragging_divider);
}

#[test]
fn non_divider_mouse_routes_to_active_mode() {
    let (mut modes, files_rec, _, _) = three_modes();
    let mut s = shell();
    s.left_rect = Rect::new(0, 0, 38, 20);
    s.handle_mouse(&mut modes, mouse(MouseEventKind::ScrollDown, 50, 5), 18, 18);
    assert_eq!(files_rec.borrow().mouse, 1);
}

#[test]
fn toggle_to_dirty_mode_reloads_it_lazily() {
    let (mut modes, files_rec, traces_rec, _) = three_modes();
    let mut s = shell();
    let vp = ReloadViewport::default();
    let theme = Theme::default();

    // Traces (inactive) becomes dirty while Files is active.
    s.dirty_since[TRACES_MODE] = Some(Instant::now());
    // No eager reload of the inactive mode.
    s.reload_active_if_due(&mut modes, vp, &theme).unwrap();
    assert_eq!(traces_rec.borrow().reloads, 0);

    // Cycling to Traces reloads it immediately.
    s.cycle(true);
    s.reload_active_if_due(&mut modes, vp, &theme).unwrap();
    assert_eq!(traces_rec.borrow().reloads, 1);
    assert!(s.dirty_since[TRACES_MODE].is_none());
    // Files was never reloaded.
    assert_eq!(files_rec.borrow().reloads, 0);
}

#[test]
fn apply_events_coalesces_repeated_resize_keys() {
    let (mut modes, _, _, _) = three_modes();
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
    s.apply_events(&mut modes, burst, vp, &theme).unwrap();
    // One step per burst, not one per repeat.
    assert_eq!(s.sidebar_pref.effective(200), initial + 4);
}

#[test]
fn toggle_is_instant_and_defers_build() {
    let (mut modes, _, _, _) = three_modes();
    let mut s = shell();
    // Pretend the Traces mode hasn't been built yet.
    s.built = [true, false, true];
    s.cycle(true);
    assert_eq!(s.active, TRACES_MODE);
    // The flip is instant; the build is deferred to build_active so the
    // loop can draw a loading frame first.
    assert!(!s.built[TRACES_MODE], "cycle must not build eagerly");
    let _ = &mut modes;
}

#[test]
fn loading_frame_shows_tab_strip_and_message() {
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
        draw_loading(
            f,
            cols[0],
            cols[1],
            TabStrip {
                active: TRACES_MODE,
            },
            &theme,
        );
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
    assert!(text.contains("Files"), "tab strip missing Files: {text:?}");
    assert!(
        text.contains("Traces"),
        "tab strip missing Traces: {text:?}"
    );
}

#[test]
fn apply_events_quit_short_circuits() {
    let (mut modes, files_rec, _, _) = three_modes();
    let mut s = shell();
    let vp = ReloadViewport::default();
    let theme = Theme::default();
    let burst = vec![
        Event::Key(key(KeyCode::Char('j'))),
        Event::Key(key(KeyCode::Char('q'))),
        Event::Key(key(KeyCode::Char('j'))),
    ];
    assert_eq!(
        s.apply_events(&mut modes, burst, vp, &theme).unwrap(),
        AppCommand::Quit
    );
    // Only the first j reached the mode.
    assert_eq!(files_rec.borrow().keys, vec![KeyCode::Char('j')]);
}
