use super::test_support::*;
use super::*;
use ratatui::{Terminal, backend::TestBackend};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn fixture(paths: &[&str]) -> (TempDir, git2::Repository) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    for path in paths {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "original\n").unwrap();
    }
    stage_all(&repo);
    commit_index(&repo, "initial");
    (dir, repo)
}

fn live(dir: &Path) -> FilesMode {
    let repo = git::Repo::discover_at(dir).unwrap();
    // Keep content resolution independent of the process cwd shared by parallel tests.
    let raw = git2::Repository::open(dir).unwrap();
    for status in repo.working_tree_snapshot().unwrap().stages {
        for path in status.paths {
            if let Ok(bytes) = fs::read(dir.join(path)) {
                raw.blob(&bytes).unwrap();
            }
        }
    }
    let (input, model) = FilesMode::try_model(&repo).unwrap();
    FilesMode::new(model, input, Some(repo), false, &theme(), 80)
}

fn finish(mode: &mut FilesMode) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while mode.action_job.is_some() {
        mode.background(true);
        assert!(Instant::now() < deadline, "action did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn refresh(mode: &mut FilesMode) {
    assert!(
        mode.take_refresh_request(),
        "action must explicitly request a refresh"
    );
    mode.reload(
        ReloadViewport {
            left_viewport: 20,
            right_viewport: 20,
            right_width: 80,
        },
        &theme(),
    )
    .unwrap();
}

#[test]
fn sidebar_a_stages_outside_the_selected_directory_and_refreshes_without_notifications() {
    let (dir, _) = fixture(&["src/a.txt", "src/nested/b.txt", "other.txt"]);
    for path in ["src/a.txt", "src/nested/b.txt", "other.txt"] {
        fs::write(dir.path().join(path), "edited\n").unwrap();
    }
    let mut mode = live(dir.path());
    assert!(mode.sidebar.select_directory_path("src/", 20));
    let before: Vec<_> = mode
        .visible_diff_window(DrawBudget::Full)
        .iter()
        .map(line_text)
        .collect();
    assert!(Mode::reserves_key(&mode, KeyCode::Char('a')));
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('2'), 20, 20);
    assert_eq!(mode.focus, Focus::Diff);
    finish(&mut mode);
    refresh(&mut mode);
    for path in ["src/a.txt", "src/nested/b.txt", "other.txt"] {
        assert!(mode.model.stages[path].is_staged());
        assert!(!mode.model.stages[path].is_unstaged());
    }
    assert_eq!(
        mode.sidebar.selected_directory_path().as_deref(),
        Some("src/")
    );
    let after: Vec<_> = mode
        .visible_diff_window(DrawBudget::Fast)
        .iter()
        .map(line_text)
        .collect();
    assert_eq!(after, before);
    assert!(!Mode::reserves_key(&mode, KeyCode::Char('a')));
    Mode::handle_key(&mut mode, KeyCode::Char('1'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    for path in ["src/a.txt", "src/nested/b.txt", "other.txt"] {
        assert!(!mode.model.stages[path].is_staged());
        assert!(mode.model.stages[path].is_unstaged());
        assert_eq!(
            fs::read_to_string(dir.path().join(path)).unwrap(),
            "edited\n"
        );
    }
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(
        repo.index().unwrap().write_tree().unwrap(),
        repo.head().unwrap().peel_to_tree().unwrap().id()
    );
}

#[test]
fn sidebar_a_uses_fresh_status_even_when_the_sidebar_is_empty() {
    let (dir, _) = fixture(&["a.txt"]);
    let mut mode = live(dir.path());
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    Mode::handle_key(&mut mode, KeyCode::Char('1'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    let repo = git2::Repository::open(dir.path()).unwrap();
    let entry = repo
        .index()
        .unwrap()
        .get_path(Path::new("a.txt"), 0)
        .unwrap();
    assert_eq!(repo.find_blob(entry.id).unwrap().content(), b"edited\n");
}

#[test]
fn sidebar_space_then_space_toggles_staging_without_watcher_events() {
    let (dir, repo) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    let before: Vec<_> = mode
        .visible_diff_window(DrawBudget::Full)
        .iter()
        .map(line_text)
        .collect();
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    let after: Vec<_> = mode
        .visible_diff_window(DrawBudget::Fast)
        .iter()
        .map(line_text)
        .collect();
    assert_eq!(after, before, "staging must retain the rendered diff");
    let status = mode.model.stages["a.txt"];
    assert!(status.is_staged() && !status.is_unstaged());
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    let entry = repo
        .index()
        .unwrap()
        .get_path(Path::new("a.txt"), 0)
        .unwrap();
    assert_eq!(repo.find_blob(entry.id).unwrap().content(), b"original\n");
    assert!(mode.model.stages["a.txt"].is_unstaged());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn directory_space_keeps_the_directory_selected_and_toggles_all_descendants() {
    let (dir, _) = fixture(&["src/a.txt", "src/nested/b.txt", "other.txt"]);
    fs::write(dir.path().join("src/a.txt"), "edited\n").unwrap();
    fs::write(dir.path().join("src/nested/b.txt"), "edited\n").unwrap();
    fs::write(dir.path().join("other.txt"), "keep\n").unwrap();
    let mut mode = live(dir.path());
    assert!(mode.sidebar.select_directory_path("src/", 20));
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert_eq!(
        mode.sidebar.selected_directory_path().as_deref(),
        Some("src/")
    );
    for path in ["src/a.txt", "src/nested/b.txt"] {
        assert!(mode.model.stages[path].is_staged());
    }
    assert!(!mode.model.stages["other.txt"].is_staged());
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    for path in ["src/a.txt", "src/nested/b.txt"] {
        assert!(!mode.model.stages[path].is_staged());
    }
}

#[test]
fn repeated_action_keys_do_not_queue_extra_mutations_and_navigation_stays_available() {
    let (dir, _) = fixture(&["a.txt", "b.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    fs::write(dir.path().join("b.txt"), "other\n").unwrap();
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    assert!(mode.captures_text_input());
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('j'), 20, 20);
    assert_eq!(mode.selected_path().unwrap().file_name().unwrap(), "b.txt");
    finish(&mut mode);
    refresh(&mut mode);
    assert!(mode.model.stages["a.txt"].is_staged());
    assert!(!mode.model.stages["b.txt"].is_staged());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn unstaged_d_requires_confirmation_and_selects_the_next_surviving_file() {
    let (dir, _) = fixture(&["a.txt", "b.txt", "c.txt"]);
    for path in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.path().join(path), "edited\n").unwrap();
    }
    let mut mode = live(dir.path());
    mode.sidebar.select_file_index(1, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    assert!(mode.captures_text_input());
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "edited\n"
    );
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    assert!(matches!(mode.input, InputState::Normal));
    refresh(&mut mode);
    assert_eq!(mode.selected_path().unwrap().file_name().unwrap(), "c.txt");
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "original\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn staged_d_opens_a_cancellable_menu_and_enter_discards_the_last_file() {
    let (dir, repo) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    stage_all(&repo);
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    assert!(mode.captures_text_input());
    let mut terminal = Terminal::new(TestBackend::new(110, 25)).unwrap();
    terminal
        .draw(|frame| {
            mode.draw(
                frame,
                Rect::new(0, 0, 30, 25),
                Rect::new(30, 0, 80, 25),
                TabStrip { active: 0 },
                ChangeLayout::Grouped,
                &theme(),
                DrawBudget::Full,
            )
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Discard all changes"));
    assert!(text.contains("Cancel"));
    assert!(text.contains("Discard unstaged changes"));
    let cells = terminal.backend().buffer().content();
    for (label, crossed_out) in [
        ("Discard all changes", false),
        ("Discard unstaged changes", true),
        ("Cancel", false),
    ] {
        let start = cells
            .windows(label.len())
            .position(|window| window.iter().map(|cell| cell.symbol()).collect::<String>() == label)
            .unwrap();
        for cell in &cells[start..start + label.len()] {
            assert_eq!(
                cell.modifier
                    .contains(ratatui::style::Modifier::CROSSED_OUT),
                crossed_out,
                "{label}"
            );
        }
    }
    Mode::handle_key(&mut mode, KeyCode::Down, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);
    assert!(!mode.captures_text_input());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "staged\n"
    );
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert!(mode.model.files.is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn cancelled_net_diff_stays_selectable_and_can_discard_only_unstaged_content() {
    let (dir, repo) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    stage_all(&repo);
    fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    let mut mode = live(dir.path());
    assert!(mode.last_input.is_empty());
    assert_eq!(mode.model.files.len(), 1);
    assert!(matches!(mode.model.bodies[0], FileBody::StatusOnly));
    let lines = mode.visible_diff_window(DrawBudget::Full);
    assert!(
        lines
            .iter()
            .any(|line| line_text(line).contains("cancel out"))
    );
    assert!(mode.diff.rows().iter().all(|row| row.anchor.is_none()));
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Down, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "staged\n"
    );
    assert!(mode.model.stages["a.txt"].is_staged());
    assert!(!mode.model.stages["a.txt"].is_unstaged());
}

#[test]
fn chained_rename_has_one_actionable_row_at_its_final_name() {
    let (dir, repo) = fixture(&["a.txt"]);
    fs::rename(dir.path().join("a.txt"), dir.path().join("middle.txt")).unwrap();
    stage_all(&repo);
    fs::rename(dir.path().join("middle.txt"), dir.path().join("final.txt")).unwrap();
    let mut mode = live(dir.path());
    assert_eq!(
        mode.model.files.len(),
        1,
        "paths: {:?}\n{}",
        mode.model
            .files
            .iter()
            .map(|f| display_path(&f.file))
            .collect::<Vec<_>>(),
        mode.last_input
    );
    assert!(mode.model.stages["final.txt"].is_staged());
    assert!(mode.model.stages["final.txt"].is_unstaged());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert!(mode.model.files.is_empty());
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("final.txt").exists());
}

#[test]
fn several_chained_renames_remain_actionable_in_one_directory() {
    let names = ["a.txt", "b.txt", "c.txt"];
    let (dir, repo) = fixture(&names);
    for name in names {
        fs::write(dir.path().join(name), format!("original {name}\n")).unwrap();
    }
    stage_all(&repo);
    commit_index(&repo, "distinct contents");
    fs::create_dir(dir.path().join("middle")).unwrap();
    fs::create_dir(dir.path().join("final")).unwrap();
    for name in names {
        fs::rename(dir.path().join(name), dir.path().join("middle").join(name)).unwrap();
    }
    stage_all(&repo);
    for name in names {
        fs::rename(
            dir.path().join("middle").join(name),
            dir.path().join("final").join(name),
        )
        .unwrap();
    }
    let mut mode = live(dir.path());
    assert_eq!(mode.model.files.len(), names.len());
    for name in names {
        assert!(
            mode.model
                .files
                .iter()
                .any(|file| display_path(&file.file) == format!("final/{name}"))
        );
    }
    assert!(mode.sidebar.select_directory_path("final/", 20));
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert!(mode.model.files.is_empty());
    for name in names {
        assert_eq!(
            fs::read_to_string(dir.path().join(name)).unwrap(),
            format!("original {name}\n")
        );
        assert!(!dir.path().join("final").join(name).exists());
    }
}

#[test]
fn piped_diff_and_diff_focus_cannot_mutate_the_repository() {
    let (dir, _) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    mode.is_static = true;
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    assert!(mode.action_job.is_none());
    assert!(
        mode.status
            .as_deref()
            .unwrap()
            .contains("repository-backed")
    );
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);
    mode.is_static = false;
    Mode::handle_key(&mut mode, KeyCode::Char('2'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('a'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    assert!(mode.action_job.is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn empty_and_externally_cleaned_selections_show_disabled_menus() {
    let (dir, _) = fixture(&["a.txt"]);
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    assert!(mode.action_job.is_none());
    assert!(mode.captures_text_input());
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    assert!(mode.captures_text_input());
    Mode::handle_key(&mut mode, KeyCode::Up, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
}

#[test]
fn cancelling_preparation_does_not_reopen_the_menu_or_write() {
    let (dir, repo) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    assert!(mode.captures_text_input());
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);
    finish(&mut mode);
    assert!(!mode.captures_text_input());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
    let index = repo.index().unwrap();
    let blob = repo
        .find_blob(index.get_path(Path::new("a.txt"), 0).unwrap().id)
        .unwrap();
    assert_eq!(blob.content(), b"original\n");
}

#[test]
fn reopening_during_preparation_cannot_receive_the_cancelled_decision() {
    let (dir, _) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Up, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Up, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn unstaged_menu_cancel_preserves_content_and_unstaged_choice_restores_it() {
    let (dir, _) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Down, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Down, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    assert!(!mode.captures_text_input());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Down, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    finish(&mut mode);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn missing_repository_and_failed_preparation_leave_discard_disabled() {
    let (dir, _) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    mode.repo = None;
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
    Mode::handle_key(&mut mode, KeyCode::Esc, 20, 20);

    let mut mode = live(dir.path());
    fs::remove_dir_all(dir.path().join(".git")).unwrap();
    Mode::handle_key(&mut mode, KeyCode::Char('d'), 20, 20);
    finish(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Up, 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Enter, 20, 20);
    assert!(mode.captures_text_input());
    assert!(mode.action_job.is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
#[ignore = "manual Space latency probe"]
fn space_latency_probe() {
    let paths: Vec<_> = (0..12).map(|index| format!("file{index:02}.rs")).collect();
    let names: Vec<_> = paths.iter().map(String::as_str).collect();
    let (dir, repo) = fixture(&names);
    let content: String = (0..500)
        .map(|index| format!("fn item_{index}() {{ let value = 2; }}\n"))
        .collect();
    for path in &paths {
        fs::write(dir.path().join(path), &content).unwrap();
    }
    repo.blob(content.as_bytes()).unwrap();
    let mut mode = live(dir.path());
    let start = Instant::now();
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    let action = start.elapsed();
    let refresh_start = Instant::now();
    refresh(&mut mode);
    eprintln!(
        "Space action={action:?}, staging refresh={:?}",
        refresh_start.elapsed()
    );
    assert!(mode.model.stages[&paths[0]].is_staged());
}

#[test]
#[ignore = "manual content reload latency probe"]
fn files_content_reload_probe() {
    let paths: Vec<_> = (0..12).map(|index| format!("file{index:02}.rs")).collect();
    let names: Vec<_> = paths.iter().map(String::as_str).collect();
    let (dir, repo) = fixture(&names);
    let content: String = (0..500)
        .map(|index| format!("fn item_{index}() {{ let value = 2; }}\n"))
        .collect();
    for path in &paths {
        fs::write(dir.path().join(path), &content).unwrap();
    }
    let mut mode = live(dir.path());
    let edited = content.replacen("value = 2", "value = 3", 1);
    fs::write(dir.path().join(&paths[0]), &edited).unwrap();
    repo.blob(edited.as_bytes()).unwrap();
    let start = Instant::now();
    assert!(
        mode.reload(
            ReloadViewport {
                left_viewport: 20,
                right_viewport: 20,
                right_width: 80,
            },
            &theme()
        )
        .unwrap()
    );
    eprintln!(
        "One edited file among twelve 500-function Rust files: content reload={:?}",
        start.elapsed()
    );
}

#[test]
fn hidden_completion_retains_its_refresh_request() {
    let (dir, _) = fixture(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let mut mode = live(dir.path());
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    let deadline = Instant::now() + Duration::from_secs(10);
    while mode.action_job.is_some() {
        mode.background(false);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    refresh(&mut mode);
    assert!(mode.model.stages["a.txt"].is_staged());
}
