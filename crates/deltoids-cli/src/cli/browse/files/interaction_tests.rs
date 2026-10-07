use super::model::FileBody;
use super::test_support::*;
use super::*;
use crate::cli::browse::comments::LineSide;
use crate::sidebar::display_path;
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
    assert_eq!(mode.shown(), Column::Staged);
    assert_eq!(mode.focus, Focus::Diff);
    let after: Vec<_> = mode
        .visible_window(Column::Staged, DrawBudget::Full)
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
        .visible_window(Column::Staged, DrawBudget::Full)
        .iter()
        .map(line_text)
        .collect();
    assert_eq!(after, before, "the staged pane shows the same diff");
    assert_eq!(mode.shown(), Column::Staged);
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
    assert!(mode.last_input.all().is_empty());
    assert_eq!(mode.model.files.len(), 1);
    assert!(matches!(mode.model.bodies[0], FileBody::StatusOnly));
    let text = |lines: Vec<ratatui::text::Line<'static>>| {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    };
    let staged = text(mode.visible_window(Column::Staged, DrawBudget::Full));
    let unstaged = text(mode.visible_window(Column::Unstaged, DrawBudget::Full));
    assert!(staged.contains("staged") && staged.contains("original"));
    assert!(unstaged.contains("staged") && unstaged.contains("original"));
    assert!(!staged.contains("cancel out"));
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
        mode.last_input.all()
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

fn screen(mode: &mut FilesMode, budget: DrawBudget) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(121, 24)).unwrap();
    term.draw(|frame| {
        Mode::draw(
            mode,
            frame,
            Rect::new(0, 0, 30, 24),
            Rect::new(30, 0, 91, 24),
            TabStrip { active: 0 },
            deltoids::ChangeLayout::Grouped,
            &theme(),
            budget,
        );
    })
    .unwrap();
    let buffer = term.backend().buffer();
    (0..24)
        .map(|y| (0..121).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

fn settle(mode: &mut FilesMode) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        screen(mode, DrawBudget::Full);
        mode.background(true);
        if !mode.staged.cache.pending() && !mode.unstaged.cache.pending() {
            return screen(mode, DrawBudget::Full);
        }
        assert!(Instant::now() < deadline, "render did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn numbered(lines: usize) -> String {
    (1..=lines).map(|n| format!("line {n}\n")).collect()
}

/// `a.txt` with a staged edit on line 2 and an unstaged edit on line 11,
/// plus `b.txt` with an unstaged edit only.
fn dual_fixture() -> (TempDir, git2::Repository) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    fs::write(dir.path().join("a.txt"), numbered(12)).unwrap();
    fs::write(dir.path().join("b.txt"), "b\n").unwrap();
    stage_all(&repo);
    commit_index(&repo, "initial");
    fs::write(
        dir.path().join("a.txt"),
        numbered(12).replace("line 2\n", "line 2 staged\n"),
    )
    .unwrap();
    stage_all(&repo);
    fs::write(
        dir.path().join("a.txt"),
        numbered(12)
            .replace("line 2\n", "line 2 staged\n")
            .replace("line 11\n", "line 11 unstaged\n"),
    )
    .unwrap();
    fs::write(dir.path().join("b.txt"), "b unstaged\n").unwrap();
    (dir, repo)
}

fn select_file(mode: &mut FilesMode, path: &str) {
    let index = mode
        .model
        .files
        .iter()
        .position(|file| display_path(&file.file) == path)
        .unwrap();
    mode.sidebar.select_file_index(index, 20);
    mode.snap_diff_to_selected_file();
}

#[test]
fn a_file_with_both_columns_shows_one_and_s_switches_to_the_other() {
    let (dir, _repo) = dual_fixture();
    let mut mode = live(dir.path());
    select_file(&mut mode, "a.txt");
    let rows = settle(&mut mode);
    assert!(rows[0].contains("[2]─Staged - Unstaged"), "{}", rows[0]);
    let body = rows.join("\n");
    assert!(body.contains("line 11 unstaged"), "unstaged is the default");
    assert!(!body.contains("line 2 staged"));

    Mode::handle_key(&mut mode, KeyCode::Char('s'), 20, 20);
    assert_eq!(mode.shown(), Column::Staged);
    let body = settle(&mut mode).join("\n");
    assert!(body.contains("line 2 staged"));
    assert!(!body.contains("line 11 unstaged"));

    // A file with one kind of change shows it, and the choice sticks for
    // the next file that has both.
    select_file(&mut mode, "b.txt");
    let rows = settle(&mut mode);
    assert!(rows[0].contains("[2]─Unstaged changes"), "{}", rows[0]);
    Mode::handle_key(&mut mode, KeyCode::Char('s'), 20, 20);
    assert_eq!(mode.shown(), Column::Unstaged, "s needs both columns");
    select_file(&mut mode, "a.txt");
    assert_eq!(mode.shown(), Column::Staged);
}

#[test]
fn staging_more_of_a_file_that_keeps_both_columns_rebuilds_both_panes() {
    let (dir, repo) = dual_fixture();
    let worktree = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    let with_middle = worktree.replace("line 6\n", "line 6 middle\n");
    fs::write(dir.path().join("a.txt"), &with_middle).unwrap();
    let mut mode = live(dir.path());
    select_file(&mut mode, "a.txt");
    assert!(
        !mode
            .visible_window(Column::Staged, DrawBudget::Full)
            .iter()
            .any(|line| line_text(line).contains("line 6 middle"))
    );

    // Stage line 6 too, as `git add -p` would; the net diff is unchanged.
    let index_text = numbered(12)
        .replace("line 2\n", "line 2 staged\n")
        .replace("line 6\n", "line 6 middle\n");
    let blob = repo.blob(index_text.as_bytes()).unwrap();
    let mut index = repo.index().unwrap();
    let mut entry = index.get_path(Path::new("a.txt"), 0).unwrap();
    entry.id = blob;
    entry.file_size = index_text.len() as u32;
    index.add(&entry).unwrap();
    index.write().unwrap();

    mode.reload(
        ReloadViewport {
            left_viewport: 20,
            right_viewport: 20,
            right_width: 80,
        },
        &theme(),
    )
    .unwrap();
    assert!(mode.model.stages["a.txt"].is_staged());
    assert!(mode.model.stages["a.txt"].is_unstaged());
    let staged: Vec<_> = mode
        .visible_window(Column::Staged, DrawBudget::Full)
        .iter()
        .map(line_text)
        .collect();
    assert!(staged.iter().any(|line| line.contains("line 6 middle")));
    let unstaged: Vec<_> = mode
        .visible_window(Column::Unstaged, DrawBudget::Full)
        .iter()
        .map(line_text)
        .collect();
    assert!(!unstaged.iter().any(|line| line.contains("+line 6")));
}

/// Put `column`'s cursor on the first diff line whose anchor matches.
fn comment_on(mode: &mut FilesMode, column: Column, side: LineSide, line: usize, note: &str) {
    mode.visible_window(column, DrawBudget::Full);
    let row = mode
        .pane(column)
        .rows()
        .iter()
        .position(|row| {
            row.place.is_some()
                && row
                    .anchor
                    .as_ref()
                    .is_some_and(|anchor| anchor.side == side && anchor.line == line)
        })
        .expect("the anchored line is on screen");
    mode.focus = Focus::Diff;
    mode.pane_mut(column).select_row(row);
    Mode::handle_key(mode, KeyCode::Char('c'), 20, 20);
    for ch in note.chars() {
        Mode::handle_key(mode, KeyCode::Char(ch), 20, 20);
    }
    Mode::handle_key(mode, KeyCode::Enter, 20, 20);
}

#[test]
fn comments_number_lines_by_the_version_each_pane_shows() {
    let (dir, _repo) = dual_fixture();
    let worktree = numbered(12)
        .replace("line 2\n", "line 2 again\n")
        .replace("line 11\n", "line 11 unstaged\n");
    fs::write(dir.path().join("a.txt"), worktree).unwrap();
    let mut mode = live(dir.path());
    select_file(&mut mode, "a.txt");

    // Line 2 is changed in both panes: staged adds index line 2, and the
    // unstaged pane removes that same index line and adds worktree line 2.
    comment_on(&mut mode, Column::Staged, LineSide::Index, 2, "index note");
    comment_on(
        &mut mode,
        Column::Unstaged,
        LineSide::New,
        2,
        "worktree note",
    );

    let index_line = CommentAnchor {
        scope: crate::cli::browse::comments::CommentScope::WorkingTree,
        path: "a.txt".to_string(),
        side: LineSide::Index,
        line: 2,
    };
    let worktree_line = CommentAnchor {
        side: LineSide::New,
        ..index_line.clone()
    };
    assert_eq!(mode.comments.note(&index_line), Some("index note"));
    assert_eq!(mode.comments.note(&worktree_line), Some("worktree note"));

    let text = |mode: &mut FilesMode, column| {
        mode.visible_window(column, DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let staged = text(&mut mode, Column::Staged);
    let unstaged = text(&mut mode, Column::Unstaged);
    assert!(staged.contains("index note") && !staged.contains("worktree note"));
    assert!(unstaged.contains("index note") && unstaged.contains("worktree note"));
}

#[test]
fn a_comment_follows_its_file_into_the_staged_pane() {
    let (dir, _repo) = dual_fixture();
    let mut mode = live(dir.path());
    select_file(&mut mode, "b.txt");
    comment_on(&mut mode, Column::Unstaged, LineSide::New, 1, "keep me");
    Mode::handle_key(&mut mode, KeyCode::Char('1'), 20, 20);
    Mode::handle_key(&mut mode, KeyCode::Char(' '), 20, 20);
    finish(&mut mode);
    refresh(&mut mode);
    assert_eq!(mode.shown(), Column::Staged);
    assert!(
        mode.visible_window(Column::Staged, DrawBudget::Full)
            .iter()
            .any(|line| line_text(line).contains("keep me"))
    );
}

#[test]
fn switching_columns_keeps_each_ones_render_and_scroll() {
    let (dir, _repo) = dual_fixture();
    let mut mode = live(dir.path());
    select_file(&mut mode, "a.txt");
    settle(&mut mode);
    Mode::handle_key(&mut mode, KeyCode::Char('J'), 20, 20);
    let unstaged_scroll = mode.unstaged.cursor.scroll;
    Mode::handle_key(&mut mode, KeyCode::Char('s'), 20, 20);
    settle(&mut mode);
    assert_eq!(
        mode.staged.cursor.scroll, 0,
        "J scrolled only the shown column"
    );
    Mode::handle_key(&mut mode, KeyCode::Char('s'), 20, 20);
    let rows = screen(&mut mode, DrawBudget::Fast);
    assert!(
        !rows.iter().any(|row| row.contains("Rendering")),
        "{}",
        rows.join("\n")
    );
    assert_eq!(mode.unstaged.cursor.scroll, unstaged_scroll);
}
