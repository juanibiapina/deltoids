use super::*;

fn events(shell: &mut Shell, mode: &mut Box<dyn Mode>, events: Vec<Event>) {
    shell
        .apply_events(mode, events, ReloadViewport::default(), &Theme::default())
        .unwrap();
}

fn refresh(shell: &mut Shell, mode: &mut Box<dyn Mode>) {
    shell.drain_watchers(mode);
    shell
        .reload_if_due(mode, ReloadViewport::default(), &Theme::default())
        .unwrap();
}

#[test]
fn idle_timeouts_draw_nothing_after_startup_and_one_settled_frame() {
    let mut shell = shell();
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
    for _ in 0..1000 {
        shell.note_input(true);
        assert!(!shell.needs_redraw());
    }
    let (mut mode, _) = one_mode();
    shell.note_input(false);
    events(
        &mut shell,
        &mut mode,
        vec![Event::Key(key(KeyCode::Char('j')))],
    );
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Fast);
    shell.note_input(true);
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
    shell.note_input(true);
    assert!(!shell.needs_redraw());
}

#[test]
fn stable_refresh_does_not_redraw_and_hidden_initialization_waits() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.take_draw_budget();
    files.borrow_mut().stable_reads = 1;
    shell.dirty_since = Some(Instant::now() - DEBOUNCE_DELAY);
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 1);
    assert!(!shell.needs_redraw());
    shell.built = false;
    events(&mut shell, &mut mode, vec![Event::FocusLost]);
    shell.build(&mut mode, ReloadViewport::default(), &Theme::default());
    assert!(!shell.built);
}

#[test]
fn hidden_windows_retain_changes_and_refresh_on_return() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.armed = true;
    shell.take_draw_budget();
    let dir = tempfile::tempdir().unwrap();
    let watcher = watch::ChangeWatcher::new(&[dir.path()]).unwrap();
    let receiver = watcher.receiver();
    shell.receiver = Some(receiver.clone());
    events(&mut shell, &mut mode, vec![Event::FocusLost]);
    receiver.request_rescan();
    for _ in 0..1000 {
        refresh(&mut shell, &mut mode);
        shell.note_input(true);
        assert!(!shell.needs_redraw());
        assert!(!shell.poll_timeout().is_zero());
    }
    assert_eq!(files.borrow().reloads, 0);
    events(&mut shell, &mut mode, vec![Event::FocusGained]);
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 1);
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 1);
}

#[test]
fn a_late_focus_loss_in_the_input_burst_prevents_eager_reload_and_draw() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.dirty_since = Some(Instant::now() - DEBOUNCE_DELAY);
    events(
        &mut shell,
        &mut mode,
        vec![
            Event::FocusGained,
            Event::Key(key(KeyCode::Char('j'))),
            Event::FocusLost,
        ],
    );
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 0);
    assert!(!shell.needs_redraw());
    assert_eq!(shell.poll_timeout(), POLL_TIMEOUT);
}

#[test]
fn a_command_does_not_discard_a_later_focus_loss() {
    let (_, files) = RecordingMode::new();
    let mut mode: Box<dyn Mode> = Box::new(RecordingMode {
        rec: files,
        selected: Some(PathBuf::from("/tmp/selected.txt")),
        capturing: false,
    });
    let mut shell = shell();
    shell.commands = vec![custom_command('e', "echo selected", false)];
    let command = shell
        .apply_events(
            &mut mode,
            vec![Event::Key(key(KeyCode::Char('e'))), Event::FocusLost],
            ReloadViewport::default(),
            &Theme::default(),
        )
        .unwrap();
    assert!(matches!(command, AppCommand::Run(_)));
    assert!(!shell.needs_redraw());
    assert!(!shell.interactive());
}

#[test]
fn clean_focus_return_repaints_without_reading_the_mode() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.armed = true;
    shell.take_draw_budget();
    events(
        &mut shell,
        &mut mode,
        vec![Event::FocusLost, Event::FocusGained],
    );
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 0);
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
    shell.note_input(true);
    assert!(!shell.needs_redraw());
}

#[test]
fn expired_read_and_watcher_retries_are_deferred_without_busy_waiting() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    files.borrow_mut().watch_failures = 1;
    shell.arm(&mut mode);
    shell.watch_retry_at = Some(Instant::now() - Duration::from_secs(2));
    shell.reload_retry_at = Some(Instant::now() - Duration::from_secs(2));
    events(&mut shell, &mut mode, vec![Event::FocusLost]);
    refresh(&mut shell, &mut mode);
    assert!(shell.watch_error.is_some());
    assert_eq!(files.borrow().reloads, 0);
    assert_eq!(shell.poll_timeout(), POLL_TIMEOUT);
    events(&mut shell, &mut mode, vec![Event::FocusGained]);
    refresh(&mut shell, &mut mode);
    assert!(shell.watch_error.is_none());
    assert_eq!(files.borrow().reloads, 1);
}

#[test]
fn key_and_mouse_input_resume_pending_work_when_focus_gain_is_missing() {
    for input in [
        Event::Key(key(KeyCode::Char('j'))),
        Event::Mouse(mouse(MouseEventKind::ScrollDown, 50, 5)),
    ] {
        let (mut mode, files) = one_mode();
        let mut shell = shell();
        shell.dirty_since = Some(Instant::now());
        events(&mut shell, &mut mode, vec![Event::FocusLost, input]);
        refresh(&mut shell, &mut mode);
        assert_eq!(files.borrow().reloads, 1);
        assert!(shell.needs_redraw());
    }
}

#[test]
fn resize_is_retained_while_hidden_and_rendered_in_full_on_return() {
    let (mut mode, _) = one_mode();
    let mut shell = shell();
    shell.take_draw_budget();
    events(
        &mut shell,
        &mut mode,
        vec![Event::FocusLost, Event::Resize(120, 40)],
    );
    assert!(!shell.needs_redraw());
    events(&mut shell, &mut mode, vec![Event::FocusGained]);
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
}

#[test]
fn foreground_return_repaints_and_services_changes_without_a_focus_report() {
    let (mut mode, files) = one_mode();
    let mut shell = shell();
    shell.take_draw_budget();
    events(&mut shell, &mut mode, vec![Event::FocusLost]);
    shell.dirty_since = Some(Instant::now());
    shell.foreground_returned();
    refresh(&mut shell, &mut mode);
    assert_eq!(files.borrow().reloads, 1);
    assert!(shell.needs_redraw());
    assert_eq!(shell.take_draw_budget(), DrawBudget::Full);
}
