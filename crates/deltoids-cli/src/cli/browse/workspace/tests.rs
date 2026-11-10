use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use deltoids::Theme;

use super::Workspace;
use crate::cli::browse::files::FilesMode;
use crate::cli::browse::mode::{Mode, Viewport};
use crate::cli::browse::test_screen::{
    LEFT, PANE_HEIGHT, RIGHT, draw, left_focused, left_text, right_focused, right_text,
};
#[cfg(unix)]
use crate::cli::browse::vcs::test_support::{fixture as but_fixture, stub_but};
use crate::test_git::TestRepo;

/// A repo on `main` with two commits, a `feature` branch forked from
/// the first with one commit of its own, and a staged edit on `main`.
/// The edit is staged so its blob resolves from the object database
/// rather than the process cwd.
///
/// main:    add greeting -> shout greeting
/// feature: add greeting -> add feature
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = TestRepo::init(dir.path());
    commit(
        dir.path(),
        &repo,
        &[("greet.rs", "fn greet() {\n    println!(\"hello\");\n}\n")],
        "add greeting",
        100,
    );
    let first = repo.rev("HEAD");
    commit(
        dir.path(),
        &repo,
        &[("greet.rs", "fn greet() {\n    println!(\"HELLO\");\n}\n")],
        "shout greeting",
        300,
    );
    repo.git(&["branch", "feature", &first]);
    repo.git(&["checkout", "-q", "-f", "feature"]);
    commit(
        dir.path(),
        &repo,
        &[("feature.rs", "fn feature() {}\n")],
        "add feature",
        200,
    );
    repo.git(&["checkout", "-q", "-f", "main"]);
    fs::write(
        dir.path().join("greet.rs"),
        "fn greet() {\n    println!(\"Hi\");\n}\n",
    )
    .unwrap();
    repo.stage_all();
    dir
}

/// Edit the staged `greet.rs` again without staging, so it has both a
/// staged and an unstaged change. The edit's blob goes into the object
/// database so it resolves without reading the process cwd.
fn edit_unstaged(dir: &Path) {
    let text = "fn greet() {\n    println!(\"Hey\");\n}\n";
    fs::write(dir.join("greet.rs"), text).unwrap();
    TestRepo::open(dir).blob(text.as_bytes());
}

fn commit(dir: &Path, repo: &TestRepo, files: &[(&str, &str)], msg: &str, seconds: i64) {
    for (path, text) in files {
        fs::write(dir.join(path), text).unwrap();
    }
    repo.stage_all();
    repo.commit_at(msg, seconds);
}

/// [`fixture`] with its staged edit committed: a clean working tree.
fn clean_fixture() -> tempfile::TempDir {
    let dir = fixture();
    let repo = TestRepo::open(dir.path());
    commit(dir.path(), &repo, &[], "say hi", 400);
    dir
}

fn reload(workspace: &mut Workspace) {
    let viewport = Viewport {
        height: PANE_HEIGHT,
        diff_width: 60,
    };
    workspace.reload(viewport, &Theme::default()).unwrap();
}

fn workspace(dir: &Path) -> Workspace {
    Workspace::new(FilesMode::discover_at(dir, &Theme::default(), 60))
}

fn press(workspace: &mut Workspace, keys: &[KeyCode]) {
    for key in keys {
        workspace.handle_key(*key, PANE_HEIGHT);
    }
}

fn click(workspace: &mut Workspace, column: u16, row: u16) {
    workspace.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
        PANE_HEIGHT,
    );
}

/// The rows of the left column, top to bottom.
fn left_rows(workspace: &mut Workspace) -> Vec<String> {
    left_text(workspace).lines().map(str::to_string).collect()
}

/// Where the cursor is: `'1'` for the uncommitted changes or an open
/// review, `'2'` for the branches.
fn current_tab(workspace: &mut Workspace) -> char {
    left_rows(workspace);
    if workspace.review.is_none() && workspace.status.cursor_in_branches() {
        '2'
    } else {
        '1'
    }
}

/// The left column's text while the cursor is in part `n`.
fn panel(workspace: &mut Workspace, n: char) -> String {
    assert_eq!(current_tab(workspace), n, "{}", left_text(workspace));
    left_text(workspace)
}

/// Move the cursor into part `n` the way a user does: leave the main
/// view, then `Tab` until it is there.
fn show(workspace: &mut Workspace, n: char) {
    if right_focused(workspace) {
        press(workspace, &[KeyCode::Esc]);
    }
    for _ in 0..2 {
        if current_tab(workspace) == n {
            return;
        }
        press(workspace, &[KeyCode::Tab]);
    }
    assert_eq!(current_tab(workspace), n, "{}", left_text(workspace));
}

/// Focus the main view the way a user does: click its title.
fn focus_main(workspace: &mut Workspace) {
    left_rows(workspace);
    click(workspace, RIGHT, 0);
    assert!(right_focused(workspace));
}

/// The side panel's title row.
fn title(workspace: &mut Workspace) -> String {
    left_rows(workspace).swap_remove(0)
}

/// The part with the cursor, while the side panel has focus.
fn focused_panel(workspace: &mut Workspace) -> Option<char> {
    left_focused(workspace).then(|| current_tab(workspace))
}

#[test]
fn the_status_panel_shows_the_changes_above_the_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    let rows = left_rows(&mut workspace);
    assert!(rows[0].starts_with(" Status"), "{}", rows[0]);
    assert!(
        rows[2].trim_end().ends_with("╭┄ [uncommitted]"),
        "{rows:#?}"
    );
    assert!(rows[3].starts_with(" ┊ "), "{rows:#?}");
    assert!(row_of(&mut workspace, "greet.rs") < row_of(&mut workspace, "feature"));
    assert_eq!(current_tab(&mut workspace), '1');
    assert!(right_text(&mut workspace).contains("Hi"));

    show(&mut workspace, '2');
    let right = right_text(&mut workspace);
    assert!(right.lines().next().unwrap().starts_with(" Log"), "{right}");
}

#[test]
fn j_and_k_cross_between_the_changes_and_the_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    press(&mut workspace, &[KeyCode::Char('j')]);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("shout greeting"));

    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("add feature"));
    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(
        right_text(&mut workspace).contains("add feature"),
        "the last row stays"
    );

    press(&mut workspace, &[KeyCode::Char('k'), KeyCode::Char('k')]);
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(right_text(&mut workspace).contains("Hi"));

    press(&mut workspace, &[KeyCode::Char('G')]);
    assert!(right_text(&mut workspace).contains("add feature"));
    press(&mut workspace, &[KeyCode::Char('g')]);
    assert_eq!(focused_panel(&mut workspace), Some('1'));
}

#[test]
fn clicking_a_row_moves_the_cursor_into_its_section() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    focus_main(&mut workspace);

    let feature = row_of(&mut workspace, "feature");
    click(&mut workspace, 10, feature);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("add feature"));

    let greet = row_of(&mut workspace, "greet.rs");
    click(&mut workspace, 10, greet);
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(right_text(&mut workspace).contains("Hi"));
}

#[test]
fn tab_keeps_each_section_selection() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Char('j')]);
    let branches = panel(&mut workspace, '2');

    show(&mut workspace, '1');
    show(&mut workspace, '2');

    assert_eq!(panel(&mut workspace, '2'), branches);
    assert!(right_text(&mut workspace).contains("add feature"));
}

#[test]
fn a_clean_tree_says_so_and_puts_the_cursor_on_the_branches() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    let left = left_text(&mut workspace);
    assert!(left.contains("╭┄ [uncommitted] (no changes)"), "{left}");
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("say hi"));
}

#[test]
fn on_a_clean_tree_the_cursor_stays_on_the_branches() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    for key in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Char('k')] {
        press(&mut workspace, &[key]);
        assert_eq!(focused_panel(&mut workspace), Some('2'), "{key:?}");
    }
}

#[test]
fn committing_every_change_moves_the_cursor_to_the_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    press(&mut workspace, &[KeyCode::Enter]);
    assert!(right_focused(&mut workspace));

    let repo = TestRepo::open(dir.path());
    commit(dir.path(), &repo, &[], "say hi", 400);
    reload(&mut workspace);

    assert!(left_text(&mut workspace).contains("(no changes)"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("say hi"));
}

#[test]
fn a_new_change_on_a_clean_tree_takes_the_cursor() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
}

#[test]
fn a_new_change_moves_focus_from_the_branches_main_view_to_the_changes() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());
    focus_main(&mut workspace);

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert!(!right_focused(&mut workspace));
    assert_eq!(focused_panel(&mut workspace), Some('1'));
}

#[test]
fn further_changes_leave_the_cursor_on_the_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[test]
fn a_review_on_a_clean_tree_takes_the_column_until_esc() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    assert!(panel(&mut workspace, '1').contains("Commit "));
    assert_eq!(focused_panel(&mut workspace), Some('1'));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(!left_text(&mut workspace).contains("Commit "));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[test]
fn the_startup_placeholder_opens_on_the_changes_once_built() {
    let dir = fixture();
    let theme = Theme::default();
    let mut workspace = Workspace::new(FilesMode::empty(&theme, 60));

    workspace.adopt_files(FilesMode::discover_at(dir.path(), &theme, 60));

    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(left_text(&mut workspace).contains("feature"));
}

#[test]
fn edits_reach_the_language_servers_while_the_terminal_is_unfocused() {
    use crate::lsp::fake;
    use serde_json::json;

    let dir = fixture();
    let mut workspace = workspace(dir.path());
    let (lsp, record) = fake::lsp(
        dir.path(),
        json!({}),
        fake::watching(json!([{ "globPattern": "**/*.rs" }])),
    );
    workspace
        .status
        .changes_mut()
        .attach_servers(Some((lsp.clone(), dir.path().to_path_buf())));
    lsp.warm(deltoids::Language::Rust);
    let unfocused_until = |workspace: &mut Workspace, done: &dyn Fn() -> bool| {
        (0..300).any(|_| {
            Mode::background(workspace, false);
            done() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        })
    };
    assert!(unfocused_until(&mut workspace, &|| record.replied("register")));

    let path = dir.path().join("greet.rs");
    fs::write(&path, "fn greet() {}\n").unwrap();
    lsp.file_events().record(
        &notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(path),
    );
    assert!(unfocused_until(&mut workspace, &|| {
        !record.params("workspace/didChangeWatchedFiles").is_empty()
    }));
}

#[test]
fn opens_on_the_files_panel_with_the_working_tree_diff() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(!right_focused(&mut workspace));
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
    let right = right_text(&mut workspace);
    assert!(right.contains("Hi"), "{right}");
}

#[test]
fn the_branches_panel_lists_local_branches_checked_out_first() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');

    let branches = panel(&mut workspace, '2');

    let main = branches.find("main").expect("main listed");
    let feature = branches.find("feature").expect("feature listed");
    assert!(main < feature, "{branches}");
}

#[test]
fn enter_lists_a_branch_commits_newest_first() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter]);

    let commits = panel(&mut workspace, '2');
    assert!(commits.contains("Commits (main)"), "{commits}");
    let newest = commits.find("shout greeting").expect("head listed");
    let oldest = commits.find("add greeting").expect("root listed");
    assert!(newest < oldest, "{commits}");
    assert!(!commits.contains("add feature"), "{commits}");
}

#[test]
fn number_keys_leave_focus_alone_and_free_for_custom_commands() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    for n in ['0', '1', '2', '3'] {
        press(&mut workspace, &[KeyCode::Char(n)]);
        assert_eq!(focused_panel(&mut workspace), Some('1'));
        assert!(!workspace.reserves_key(KeyCode::Char(n)));
    }
}

#[test]
fn tab_and_shift_tab_switch_between_the_side_panels() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    press(&mut workspace, &[KeyCode::Tab]);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    press(&mut workspace, &[KeyCode::Tab]);
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    press(&mut workspace, &[KeyCode::BackTab]);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(!right_focused(&mut workspace));
}

#[test]
fn h_l_and_arrows_do_not_switch_side_panels() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    for key in [
        KeyCode::Char('l'),
        KeyCode::Char('h'),
        KeyCode::Right,
        KeyCode::Left,
    ] {
        press(&mut workspace, &[key]);
        assert_eq!(focused_panel(&mut workspace), Some('1'), "{key:?}");
    }
}

#[test]
fn enter_on_a_file_focuses_its_diff_and_esc_comes_back() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    press(&mut workspace, &[KeyCode::Enter]);
    assert!(right_focused(&mut workspace));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(!right_focused(&mut workspace));
    assert_eq!(focused_panel(&mut workspace), Some('1'));
}

#[test]
fn enter_on_a_commit_file_focuses_its_diff() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    assert!(panel(&mut workspace, '1').contains("Commit "));
    assert!(!right_focused(&mut workspace));

    press(&mut workspace, &[KeyCode::Enter]);
    assert!(right_focused(&mut workspace));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(!right_focused(&mut workspace));
    // Still in the commit's files: Esc left the diff, not the commit.
    assert!(panel(&mut workspace, '1').contains("Commit "));
}

#[test]
fn tab_in_the_main_view_switches_staged_and_unstaged() {
    let dir = fixture();
    edit_unstaged(dir.path());
    let mut workspace = workspace(dir.path());
    focus_main(&mut workspace);
    let unstaged = right_text(&mut workspace);
    assert!(unstaged.contains("Hey"), "{unstaged}");
    assert!(!unstaged.contains("HELLO"), "{unstaged}");

    press(&mut workspace, &[KeyCode::Tab]);
    let staged = right_text(&mut workspace);
    assert!(staged.contains("HELLO"), "{staged}");
    assert!(!staged.contains("Hey"), "{staged}");
    assert!(right_focused(&mut workspace));

    press(&mut workspace, &[KeyCode::BackTab]);
    assert!(right_text(&mut workspace).contains("Hey"));
}

#[test]
fn tab_in_the_main_view_never_leaves_it() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    focus_main(&mut workspace);

    press(&mut workspace, &[KeyCode::Tab]);

    assert!(right_focused(&mut workspace));
    press(&mut workspace, &[KeyCode::Esc]);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[test]
fn the_main_view_shows_the_focused_branch_log() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    show(&mut workspace, '2');
    let right = right_text(&mut workspace);
    assert!(right.contains("shout greeting"), "{right}");
    assert!(right.contains("add greeting"), "{right}");

    press(&mut workspace, &[KeyCode::Char('j')]);
    let right = right_text(&mut workspace);
    assert!(right.contains("add feature"), "{right}");
    assert!(!right.contains("shout greeting"), "{right}");
}

#[test]
fn keys_in_the_focused_log_scroll_it() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    focus_main(&mut workspace);

    press(&mut workspace, &[KeyCode::Char('j')]);

    let right = right_text(&mut workspace);
    assert!(!right.contains("shout greeting"), "{right}");
    assert!(right.contains("add greeting"), "{right}");
    // Still main's branch: the side selection did not move.
    assert!(!right.contains("add feature"), "{right}");
}

#[test]
fn the_main_view_shows_the_selected_commit_diff() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter]);
    let right = right_text(&mut workspace);
    assert!(right.contains("HELLO"), "{right}");

    press(&mut workspace, &[KeyCode::Char('j')]);
    let right = right_text(&mut workspace);
    assert!(right.contains("hello"), "{right}");
    assert!(!right.contains("HELLO"), "{right}");
}

#[test]
fn a_preview_that_finishes_after_enter_lets_the_screen_settle() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter]);
    // One frame starts the selected commit's preview; Enter opens the
    // review before the preview is ever drawn again.
    draw(&mut workspace);
    press(&mut workspace, &[KeyCode::Enter]);

    assert!(panel(&mut workspace, '1').contains("Commit "));
}

#[test]
fn enter_on_a_branch_lists_its_commits_and_enter_reviews_one_in_files() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Enter]);

    let commits = panel(&mut workspace, '2');
    assert!(commits.contains("Commits (feature)"), "{commits}");
    assert!(commits.contains("add feature"), "{commits}");
    assert!(right_text(&mut workspace).contains("fn feature()"));

    press(&mut workspace, &[KeyCode::Enter]);
    let files = panel(&mut workspace, '1');
    let short = feature_tip(dir.path());
    assert!(files.contains(&format!("Commit {short}")), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("fn feature()"));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(title(&mut workspace).starts_with(" Commits (feature)"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    press(&mut workspace, &[KeyCode::Esc]);
    assert!(title(&mut workspace).starts_with(" Status"));
    // The branch selection survived the round trip.
    assert!(right_text(&mut workspace).contains("add feature"));
    press(&mut workspace, &[KeyCode::Esc]);
    assert!(title(&mut workspace).starts_with(" Status"));
}

/// [`fixture`] with one more commit on `main` whose message has a
/// subject and a body, so its review has details to show.
fn explained_fixture(body: &str) -> tempfile::TempDir {
    let dir = fixture();
    let repo = TestRepo::open(dir.path());
    let message = format!("explain the change\n\n{body}\n");
    commit(
        dir.path(),
        &repo,
        &[("greet.rs", "fn greet() {\n    println!(\"Hi\");\n}\n")],
        &message,
        400,
    );
    dir
}

fn head_short(dir: &Path) -> String {
    let repo = TestRepo::open(dir);
    let id = repo.rev("HEAD");
    id[..7].to_string()
}

#[test]
fn a_commit_review_opens_on_its_commit_row_and_header() {
    let dir = explained_fixture("The body says why the change matters.");
    let mut workspace = workspace(dir.path());

    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);

    let short = head_short(dir.path());
    let files = panel(&mut workspace, '1');
    assert!(files.starts_with(&format!(" Commit {short}")), "{files}");
    let rows = panel_lines(&mut workspace, '1');
    assert!(rows[0].ends_with("explain the change"), "{rows:#?}");
    assert!(!rows[0].contains(&short), "{rows:#?}");
    let lines: Vec<&str> = files.lines().collect();
    assert!(
        !lines[3].trim().is_empty() && lines[3].trim().chars().all(|c| c == '─'),
        "{files}"
    );
    let header = right_text(&mut workspace);
    assert!(
        header.contains("Author: Ada Lovelace <ada@example.com>"),
        "{header}"
    );
    assert!(header.contains("The body says why the change"), "{header}");
    assert!(!header.contains("println!"), "{header}");
}

#[test]
fn closing_a_commit_review_brings_back_the_commit_list() {
    let dir = explained_fixture("Why.");
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(
        &mut workspace,
        &[KeyCode::Enter, KeyCode::Char('j'), KeyCode::Enter],
    );
    press(&mut workspace, &[KeyCode::Esc]);

    let commits = panel(&mut workspace, '2');
    assert!(commits.contains("Commits (main)"), "{commits}");
    assert!(commits.contains("shout greeting"), "{commits}");
    show(&mut workspace, '2');
    let preview = right_text(&mut workspace);
    assert!(preview.contains("shout greeting"), "{preview}");
    assert!(!preview.contains("explain the change"), "{preview}");
}

#[test]
fn a_review_is_the_only_tab_until_esc_returns_to_the_reviewed_row() {
    let dir = explained_fixture("Why.");
    let mut workspace = workspace(dir.path());
    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    let short = head_short(dir.path());
    assert_eq!(current_tab(&mut workspace), '1');
    assert!(!left_text(&mut workspace).contains("[uncommitted]"));
    assert!(title(&mut workspace).starts_with(&format!(" Commit {short}  ")));

    for key in [KeyCode::Tab, KeyCode::BackTab] {
        press(&mut workspace, &[key]);
        assert_eq!(focused_panel(&mut workspace), Some('1'), "{key:?}");
    }

    press(&mut workspace, &[KeyCode::Esc]);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(panel(&mut workspace, '2').contains("Commits (main)"));
    let preview = right_text(&mut workspace);
    assert!(preview.contains("explain the change"), "{preview}");
}

fn feature_tip(dir: &Path) -> String {
    let repo = TestRepo::open(dir);
    let id = repo.rev("feature");
    id[..7].to_string()
}

#[test]
fn clicking_the_main_view_focuses_it_and_esc_returns_to_the_side() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter]);
    focus_main(&mut workspace);
    assert!(right_focused(&mut workspace));

    // `j` walks the diff, not the commit list.
    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("HELLO"));

    press(&mut workspace, &[KeyCode::Esc, KeyCode::Char('j')]);
    assert!(!right_focused(&mut workspace));
    let right = right_text(&mut workspace);
    assert!(right.contains("hello"), "{right}");
    assert!(!right.contains("HELLO"), "{right}");
}

#[test]
fn clicking_a_panel_row_focuses_the_panel_and_selects_the_row() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    focus_main(&mut workspace);
    let rows = left_rows(&mut workspace);
    let feature = rows
        .iter()
        .position(|row| row.contains("feature") && !row.contains("add feature"))
        .expect("feature branch on screen") as u16;

    click(&mut workspace, 5, feature);

    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("add feature"));
}

/// The screen row of the left column that contains `text`.
fn row_of(workspace: &mut Workspace, text: &str) -> u16 {
    let rows = left_rows(workspace);
    rows.iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("no {text:?} on screen:\n{}", rows.join("\n"))) as u16
}

#[test]
fn double_clicking_a_commit_opens_it_like_enter() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Enter]);
    let commit = row_of(&mut workspace, "add feature");

    click(&mut workspace, 5, commit);
    assert!(title(&mut workspace).starts_with(" Commits (feature)"));
    assert_eq!(current_tab(&mut workspace), '2');
    click(&mut workspace, 5, commit);

    let files = panel(&mut workspace, '1');
    let short = feature_tip(dir.path());
    assert!(files.contains(&format!("Commit {short}")), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
}

#[test]
fn double_clicking_a_branch_lists_its_commits() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    let branch = row_of(&mut workspace, "  feature");

    click(&mut workspace, 5, branch);
    click(&mut workspace, 5, branch);

    let commits = panel(&mut workspace, '2');
    assert!(commits.contains("Commits (feature)"), "{commits}");
}

#[test]
fn a_comment_editor_takes_digits_and_tab_as_text() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    right_text(&mut workspace);
    focus_main(&mut workspace);
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Char('c')]);
    assert!(workspace.captures_text_input());

    press(&mut workspace, &[KeyCode::Char('2'), KeyCode::Tab]);

    assert!(workspace.captures_text_input());
    assert!(right_focused(&mut workspace));
}

#[test]
fn h_and_l_are_reserved_over_custom_commands_only_in_the_diff() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    for key in ['h', 'l'] {
        assert!(!workspace.reserves_key(KeyCode::Char(key)), "{key}");
    }
    press(&mut workspace, &[KeyCode::Enter]);
    assert!(right_focused(&mut workspace));
    for key in ['h', 'l'] {
        assert!(workspace.reserves_key(KeyCode::Char(key)), "{key}");
    }
}

#[test]
fn outside_a_repository_the_branches_panel_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut workspace = workspace(dir.path());

    assert!(panel(&mut workspace, '1').contains("Not a git repository"));
}

#[test]
fn a_reload_picks_up_new_commits_and_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    assert!(!right_text(&mut workspace).contains("say hi"));
    let repo = TestRepo::open(dir.path());
    commit(dir.path(), &repo, &[], "say hi", 400);
    repo.git(&["branch", "fresh"]);

    let viewport = Viewport {
        height: PANE_HEIGHT,
        diff_width: 60,
    };
    workspace.reload(viewport, &Theme::default()).unwrap();

    // The checked-out branch's log, in the main view.
    assert!(right_text(&mut workspace).contains("say hi"));
    assert!(panel(&mut workspace, '2').contains("fresh"));
}

#[test]
fn a_commit_review_shows_the_header_and_a_summary_then_follows_its_file_tree() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Enter]);
    assert!(right_text(&mut workspace).contains("Author: Ada Lovelace"));

    press(&mut workspace, &[KeyCode::Enter]);
    let header = right_text(&mut workspace);
    assert!(header.contains("Author: Ada Lovelace"), "{header}");
    assert!(header.contains("1 file changed"), "{header}");
    assert!(header.contains("+ fn feature()"), "{header}");

    press(&mut workspace, &[KeyCode::Char('j')]);
    let diff = right_text(&mut workspace);
    assert!(!diff.contains("Author:"), "{diff}");
    assert!(diff.contains("fn feature()"), "{diff}");
}

#[cfg(unix)]
fn enter_gitbutler(dir: &Path) {
    crate::cli::browse::vcs::test_support::adopt(dir);
}

/// A workspace whose branches panel runs a `but` stand-in answering
/// `status` with the JSON at `status`.
#[cfg(unix)]
fn gitbutler_workspace(dir: &Path, stubs: &Path, status: &Path) -> Workspace {
    let but = stub_but(stubs, status).into_os_string();
    Workspace::with_but(
        FilesMode::discover_at(dir, &Theme::default(), 60),
        Some(but),
    )
}

/// The body rows of panel `[n]`, title row, rule, left inset, and
/// trailing blanks removed.
fn panel_lines(workspace: &mut Workspace, n: char) -> Vec<String> {
    let text = panel(workspace, n);
    let rows: Vec<&str> = text.lines().collect();
    let mut lines: Vec<String> = rows[2..]
        .iter()
        .map(|row| {
            let chars: Vec<char> = row.chars().collect();
            chars[1..].iter().collect::<String>().trim_end().to_string()
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(unix)]
#[test]
fn a_gitbutler_workspace_panel_reads_like_but_status() {
    let dir = fixture();
    enter_gitbutler(dir.path());
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = gitbutler_workspace(
        dir.path(),
        stubs.path(),
        &but_fixture("but-status-stacks.json"),
    );
    show(&mut workspace, '2');

    let lines = panel_lines(&mut workspace, '2');
    let first_stack = lines.iter().position(|l| l.starts_with("┊╭┄")).unwrap();
    let shown = &lines[first_stack..];

    assert!(lines[0].starts_with("╭┄ [uncommitted]"), "{lines:#?}");
    let reference = fs::read_to_string(but_fixture("but-status-stacks.txt")).unwrap();
    let fit = |line: &str| {
        let inner = LEFT as usize - 2;
        line.chars()
            .take(inner)
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    let expected: Vec<String> = reference.lines().map(fit).take(shown.len()).collect();
    assert_eq!(
        shown[..expected.len()],
        expected[..],
        "{}",
        shown.join("\n")
    );

    press(&mut workspace, &[KeyCode::Char('G')]);
    let bottom = panel_lines(&mut workspace, '2');
    let tail: Vec<String> = reference
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .chain([
            "┴ 3917b63 (common base, origin/main) 2026-10-14 init",
            "",
            "unapplied",
        ])
        .map(fit)
        .collect();
    let (parked, bottom) = bottom.split_last().unwrap();
    assert!(parked.ends_with("  parked"), "{parked}");
    let tail = &tail[tail.len().saturating_sub(bottom.len())..];
    assert_eq!(
        bottom[bottom.len() - tail.len()..],
        tail[..],
        "{}",
        bottom.join("\n")
    );
}

#[cfg(unix)]
#[test]
fn the_cursor_skips_rows_that_are_not_branches_or_commits() {
    let dir = fixture();
    enter_gitbutler(dir.path());
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = gitbutler_workspace(
        dir.path(),
        stubs.path(),
        &but_fixture("but-status-stacks.json"),
    );
    show(&mut workspace, '2');

    // From [empty], past its stack's end, to the next stack's branch.
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Enter]);

    assert!(panel(&mut workspace, '1').contains("Review top-feature"));
}

/// The fixture repo in a GitButler workspace with `feature` applied:
/// `but status` lists its one commit, by its real id.
#[cfg(unix)]
fn feature_status(dir: &Path) -> std::path::PathBuf {
    let repo = TestRepo::open(dir);
    let feature = repo.rev("feature");
    let branch = status_branch("fe", "feature", &[("abcd", &feature, "add feature")]);
    write_status(dir, &[&[branch]])
}

/// Write `but status` JSON into the fixture repo's `.git`: `stacks` of
/// [`status_branch`]es, top first, all resting on `feature`'s parent.
#[cfg(unix)]
fn write_status(dir: &Path, stacks: &[&[String]]) -> std::path::PathBuf {
    let repo = TestRepo::open(dir);
    let base = repo.rev("feature~1");
    let stacks: Vec<String> = stacks
        .iter()
        .enumerate()
        .map(|(index, branches)| {
            format!(
                r#"{{"cliId": "g{index}", "assignedChanges": [], "branches": [{}]}}"#,
                branches.join(",")
            )
        })
        .collect();
    let json = format!(
        r#"{{
  "uncommittedChanges": [],
  "stacks": [{}],
  "mergeBase": {{"cliId": "", "commitId": "{base}",
    "createdAt": "1970-01-01T00:01:40+00:00", "message": "add greeting",
    "authorName": "Ada Lovelace", "authorEmail": "ada@example.com"}},
  "upstreamState": {{"behind": 0, "latestCommit": null, "lastFetched": null}}
}}"#,
        stacks.join(",")
    );
    let path = dir.join(".git").join("but-status.json");
    fs::write(&path, json).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn a_commit_row_previews_its_diff_and_enter_reviews_it_in_files() {
    let dir = fixture();
    enter_gitbutler(dir.path());
    let stubs = tempfile::tempdir().unwrap();
    let status = feature_status(dir.path());
    let mut workspace = gitbutler_workspace(dir.path(), stubs.path(), &status);
    show(&mut workspace, '2');

    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("fn feature()"));

    press(&mut workspace, &[KeyCode::Enter]);
    let files = panel(&mut workspace, '1');
    assert!(files.contains("Commit "), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
    assert!(!files.contains("greet.rs"), "{files}");
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(right_text(&mut workspace).contains("Author: Ada Lovelace"));
    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("fn feature()"));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(title(&mut workspace).starts_with(" Status"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    show(&mut workspace, '1');
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
}

#[cfg(unix)]
#[test]
fn double_clicking_a_workspace_commit_reviews_it_in_files() {
    let dir = fixture();
    enter_gitbutler(dir.path());
    let stubs = tempfile::tempdir().unwrap();
    let status = feature_status(dir.path());
    let mut workspace = gitbutler_workspace(dir.path(), stubs.path(), &status);
    show(&mut workspace, '2');
    let commit = row_of(&mut workspace, "abcd add feature");

    click(&mut workspace, 5, commit);
    assert!(title(&mut workspace).starts_with(" Status"));
    click(&mut workspace, 5, commit);

    let files = panel(&mut workspace, '1');
    assert!(files.contains("Commit "), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
    assert!(!files.contains("greet.rs"), "{files}");
    assert_eq!(focused_panel(&mut workspace), Some('1'));
}

#[cfg(unix)]
#[test]
fn double_clicking_a_workspace_branch_reviews_it_in_files() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());
    let branch = row_of(&mut workspace, "[feature]");

    click(&mut workspace, 5, branch);
    click(&mut workspace, 5, branch);

    let files = panel(&mut workspace, '1');
    assert!(files.contains("Review feature"), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(!left_text(&mut workspace).contains("[uncommitted]"));
}

/// Commit `files` on top of `branch` without touching the working tree,
/// and move `branch` to the new commit.
#[cfg(unix)]
fn commit_on(repo: &TestRepo, branch: &str, files: &[(&str, &str)], msg: &str) -> String {
    repo.commit_files_on(branch, files, msg, 400)
}

/// One `but status` branch: its CLI ID, name, and commits newest first
/// as (CLI ID, commit id, message).
#[cfg(unix)]
fn status_branch(cli_id: &str, name: &str, commits: &[(&str, &str, &str)]) -> String {
    let commits: Vec<String> = commits
        .iter()
        .map(|(cli_id, id, message)| {
            format!(
                r#"{{"cliId": "{cli_id}", "commitId": "{id}",
                  "createdAt": "1970-01-01T00:06:40+00:00", "message": "{message}",
                  "authorName": "Ada Lovelace", "authorEmail": "ada@example.com"}}"#
            )
        })
        .collect();
    format!(
        r#"{{"cliId": "{cli_id}", "name": "{name}", "branchStatus": "completelyUnpushed",
            "commits": [{}]}}"#,
        commits.join(",")
    )
}

/// The fixture repo in a GitButler workspace with two stacks: `upper`
/// (two commits: add `upper.rs`, then rewrite it) on `feature` (adds
/// `feature.rs`), and `empty` with no commits. `parked` is unapplied.
#[cfg(unix)]
fn stacked_status(dir: &Path) -> std::path::PathBuf {
    let repo = TestRepo::open(dir);
    let feature = repo.rev("feature");
    repo.git(&["branch", "upper", &feature]);
    let draft = commit_on(&repo, "upper", &[("upper.rs", "draft\n")], "draft upper");
    let last = commit_on(
        &repo,
        "upper",
        &[("upper.rs", "final\n"), ("upper.md", "# Upper\n")],
        "finish upper",
    );
    let upper = status_branch(
        "up",
        "upper",
        &[("u2", &last, "finish upper"), ("u1", &draft, "draft upper")],
    );
    let lower = status_branch("fe", "feature", &[("f1", &feature, "add feature")]);
    let empty = status_branch("em", "empty", &[]);
    write_status(dir, &[&[upper, lower], &[empty]])
}

/// A workspace on the stacked fixture with the Workspace panel focused
/// on `upper`, its first row.
#[cfg(unix)]
fn stacked_workspace(dir: &Path, stubs: &Path) -> Workspace {
    enter_gitbutler(dir);
    let status = stacked_status(dir);
    let mut workspace = gitbutler_workspace(dir, stubs, &status);
    show(&mut workspace, '2');
    workspace
}

#[cfg(unix)]
#[test]
fn enter_on_a_stacked_branch_reviews_its_own_commits_in_files() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    press(&mut workspace, &[KeyCode::Enter]);

    let files = panel(&mut workspace, '1');
    assert!(files.contains("Review upper"), "{files}");
    assert!(files.contains("upper.rs"), "{files}");
    assert!(!files.contains("feature.rs"), "{files}");
    assert!(!files.contains("greet.rs"), "{files}");
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(files.contains("upper.md"), "{files}");
    let rows = panel_lines(&mut workspace, '1');
    assert!(rows[0].contains("upper · 2 commits"), "{rows:#?}");

    let header = right_text(&mut workspace);
    let at = |text: &str| header.find(text);
    assert!(at("finish upper") < at("draft upper"), "{header}");
    assert!(at("draft upper").is_some(), "{header}");
    assert_eq!(at("add feature"), None, "{header}");

    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Char('j')]);
    let right = right_text(&mut workspace);
    assert!(right.contains("final"), "{right}");
    assert!(!right.contains("draft"), "{right}");
}

#[cfg(unix)]
#[test]
fn enter_on_a_lower_branch_reviews_only_its_commits() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    let to_feature = [KeyCode::Char('j'); 3];
    press(&mut workspace, &to_feature);
    press(&mut workspace, &[KeyCode::Enter]);

    let files = panel(&mut workspace, '1');
    assert!(files.contains("Review feature"), "{files}");
    assert!(files.contains("feature.rs"), "{files}");
    assert!(!files.contains("upper.rs"), "{files}");
}

#[cfg(unix)]
#[test]
fn a_review_keeps_its_comments_until_esc_brings_back_the_working_tree() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());
    press(&mut workspace, &[KeyCode::Enter]);
    panel(&mut workspace, '1');

    // Staging and discarding do nothing to a review.
    press(&mut workspace, &[KeyCode::Char(' '), KeyCode::Char('d')]);
    assert!(panel(&mut workspace, '1').contains("Review upper"));

    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Enter]);
    right_text(&mut workspace);
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Char('c')]);
    for ch in "why final".chars() {
        press(&mut workspace, &[KeyCode::Char(ch)]);
    }
    press(&mut workspace, &[KeyCode::Enter]);
    let copied =
        |workspace: &mut Workspace| match workspace.handle_key(KeyCode::Char('y'), PANE_HEIGHT) {
            crate::cli::browse::mode::AppCommand::CopyToClipboard(text) => text,
            other => panic!("expected a copy, got {other:?}"),
        };
    assert!(copied(&mut workspace).contains("why final"));

    press(&mut workspace, &[KeyCode::Esc, KeyCode::Esc]);
    assert!(title(&mut workspace).starts_with(" Status"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    show(&mut workspace, '1');
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
}

#[cfg(unix)]
#[test]
fn enter_on_an_empty_branch_does_nothing() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    let to_empty = [KeyCode::Char('j'); 5];
    press(&mut workspace, &to_empty);
    assert!(left_text(&mut workspace).contains("[empty]"));
    press(&mut workspace, &[KeyCode::Enter]);

    assert!(title(&mut workspace).starts_with(" Status"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[cfg(unix)]
#[test]
fn enter_on_an_unapplied_branch_lists_its_commits() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    press(&mut workspace, &[KeyCode::Char('G'), KeyCode::Enter]);

    assert!(title(&mut workspace).starts_with(" Commits (parked)"));
}

#[cfg(unix)]
#[test]
fn a_gitbutler_branch_log_stops_at_the_base() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    // [feature]: its own commit, then the base it rests on.
    press(&mut workspace, &[KeyCode::Char('j'); 3]);
    let log = right_text(&mut workspace);

    assert!(log.contains("add feature"), "{log}");
    assert!(!log.contains("add greeting"), "{log}");
}

#[cfg(unix)]
#[test]
fn a_commit_preview_reads_like_git_show() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    press(&mut workspace, &[KeyCode::Char('j')]);
    // The border names the file under the cursor; read the body below it.
    let screen = right_text(&mut workspace);
    let preview = screen
        .split_once('\n')
        .map_or("", |(_, body)| body)
        .to_string();

    let at = |text: &str| {
        preview.find(text).unwrap_or_else(|| {
            panic!(
                "no {text:?} in:
{preview}"
            )
        })
    };
    assert!(at("commit ") < at("Author: Ada Lovelace <ada@example.com>"));
    assert!(at("Date:   ") < at("    finish upper"));
    assert!(at("    finish upper") < at("upper.md"));
    assert!(at("upper.md") < at("upper.rs"));
}

/// [`fixture`] with `origin/main` at `main`'s first commit, so `main`
/// has one commit of its own past its base.
fn based_fixture() -> tempfile::TempDir {
    let dir = fixture();
    let repo = TestRepo::open(dir.path());
    let first = repo.rev("main~1");
    repo.git(&["update-ref", "refs/remotes/origin/main", &first]);
    dir
}

#[test]
fn a_git_branch_with_a_base_reads_like_but_status() {
    let dir = based_fixture();
    let mut workspace = workspace(dir.path());
    let repo = TestRepo::open(dir.path());
    let shout = repo.rev("main")[..7].to_string();
    let base = repo.rev("main~1")[..7].to_string();

    let lines = panel_lines(&mut workspace, '1');

    let stack = lines.iter().position(|l| l.starts_with("┊╭┄")).unwrap();
    assert_eq!(lines[stack], "┊╭┄ [main]", "{lines:#?}");
    assert_eq!(lines[stack + 1], format!("┊●   {shout} shout greeting"));
    assert_eq!(lines[stack + 2], "├╯");
    assert!(
        lines[stack + 4].starts_with(&format!("┴ {base} (common base, origin/main)")),
        "{lines:#?}"
    );
    assert_eq!(lines[stack + 6], "branches");
    assert!(lines[stack + 7].ends_with("  feature"), "{lines:#?}");
}

#[test]
fn enter_on_a_git_branch_with_a_base_reviews_its_commits() {
    let dir = based_fixture();
    let mut workspace = workspace(dir.path());
    press(&mut workspace, &[KeyCode::Char('j')]);
    assert!(right_text(&mut workspace).contains("shout greeting"));

    press(&mut workspace, &[KeyCode::Enter]);

    let review = panel(&mut workspace, '1');
    assert!(review.contains("Review main"), "{review}");
    assert!(review.contains("greet.rs"), "{review}");
}

#[test]
fn reviewed_marks_outlive_the_commit_review_they_were_made_in() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());
    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
    press(&mut workspace, &[KeyCode::Char('j'), KeyCode::Char('v')]);
    assert!(
        title(&mut workspace).contains("✓ 1"),
        "{}",
        title(&mut workspace)
    );

    press(
        &mut workspace,
        &[KeyCode::Esc, KeyCode::Enter, KeyCode::Enter],
    );
    let rows = panel(&mut workspace, '1');
    assert!(rows.contains("Commit "), "{rows}");
    assert!(!rows.contains("greet.rs"), "{rows}");
    assert!(
        title(&mut workspace).contains("✓ 1"),
        "{}",
        title(&mut workspace)
    );
}

#[test]
fn a_file_reviewed_in_the_working_tree_stays_reviewed_in_its_commit() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
    press(&mut workspace, &[KeyCode::Char('v')]);
    assert!(!panel(&mut workspace, '1').contains("greet.rs"));

    TestRepo::open(dir.path()).commit_at("say hi", 400);
    reload(&mut workspace);
    show(&mut workspace, '2');
    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    let rows = panel(&mut workspace, '1');
    assert!(rows.contains("say hi"), "{rows}");
    assert!(!rows.contains("greet.rs"), "{rows}");
}
