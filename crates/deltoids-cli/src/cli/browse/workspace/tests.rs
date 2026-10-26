use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use git2::{Repository, Signature, Time};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use super::{Side, Workspace};
use crate::cli::browse::files::FilesMode;
use crate::cli::browse::mode::{Mode, Viewport};
use crate::cli::browse::test_screen::{
    LEFT, PANE_HEIGHT, draw, left_border_focused, left_text, right_focused, right_text,
};
#[cfg(unix)]
use crate::cli::browse::vcs::test_support::{fixture as but_fixture, stub_but};

/// A repo on `main` with two commits, a `feature` branch forked from
/// the first with one commit of its own, and a staged edit on `main`.
/// The edit is staged so its blob resolves from the object database
/// rather than the process cwd.
///
/// main:    add greeting -> shout greeting
/// feature: add greeting -> add feature
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    commit(
        dir.path(),
        &repo,
        &[("greet.rs", "fn greet() {\n    println!(\"hello\");\n}\n")],
        "add greeting",
        100,
    );
    let first = repo.head().unwrap().peel_to_commit().unwrap();
    commit(
        dir.path(),
        &repo,
        &[("greet.rs", "fn greet() {\n    println!(\"HELLO\");\n}\n")],
        "shout greeting",
        300,
    );
    repo.branch("feature", &first, false).unwrap();
    repo.set_head("refs/heads/feature").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    commit(
        dir.path(),
        &repo,
        &[("feature.rs", "fn feature() {}\n")],
        "add feature",
        200,
    );
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(
        dir.path().join("greet.rs"),
        "fn greet() {\n    println!(\"Hi\");\n}\n",
    )
    .unwrap();
    stage_all(&repo);
    dir
}

/// Edit the staged `greet.rs` again without staging, so it has both a
/// staged and an unstaged change. The edit's blob goes into the object
/// database so it resolves without reading the process cwd.
fn edit_unstaged(dir: &Path) {
    let text = "fn greet() {\n    println!(\"Hey\");\n}\n";
    fs::write(dir.join("greet.rs"), text).unwrap();
    Repository::open(dir)
        .unwrap()
        .blob(text.as_bytes())
        .unwrap();
}

fn stage_all(repo: &Repository) -> git2::Oid {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    index.write_tree().unwrap()
}

fn commit(dir: &Path, repo: &Repository, files: &[(&str, &str)], msg: &str, seconds: i64) {
    for (path, text) in files {
        fs::write(dir.join(path), text).unwrap();
    }
    let tree = repo.find_tree(stage_all(repo)).unwrap();
    let sig = Signature::new("Ada Lovelace", "ada@example.com", &Time::new(seconds, 0)).unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
        .unwrap();
}

/// [`fixture`] with its staged edit committed: a clean working tree.
fn clean_fixture() -> tempfile::TempDir {
    let dir = fixture();
    let repo = Repository::open(dir.path()).unwrap();
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

/// The first column of side panel `n`'s tab on the top border: `'1'`
/// for Files, `'2'` for Branches.
fn tab_column(workspace: &mut Workspace, n: char) -> Option<u16> {
    let side = match n {
        '1' => Side::Files,
        '2' => Side::Branches,
        _ => panic!("no side panel {n}"),
    };
    left_rows(workspace);
    workspace
        .tab_hits
        .iter()
        .find(|(s, _)| *s == side)
        .map(|(_, columns)| columns.start)
}

/// Whether the side panel's top border has a tab for panel `n`.
fn has_panel(workspace: &mut Workspace, n: char) -> bool {
    tab_column(workspace, n).is_some()
}

/// The side panel the column shows: the tab drawn in the accent.
fn current_tab(workspace: &mut Workspace) -> char {
    let accent = rgb_to_color(Theme::default().border_active);
    ['1', '2']
        .into_iter()
        .find(|&n| {
            tab_column(workspace, n).is_some_and(|column| draw(workspace)[(column, 0)].fg == accent)
        })
        .unwrap_or_else(|| panic!("no current tab:\n{}", left_text(workspace)))
}

/// The text of side panel `n`, which must be the one shown.
fn panel(workspace: &mut Workspace, n: char) -> String {
    assert_eq!(current_tab(workspace), n, "{}", left_text(workspace));
    left_text(workspace)
}

/// Show side panel `n` the way a user does: leave the main view, then
/// `Tab` until its tab is current.
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

/// Focus the main view the way a user does: click its top border.
fn focus_main(workspace: &mut Workspace) {
    left_rows(workspace);
    click(workspace, LEFT + 1, 0);
    assert!(right_focused(workspace));
}

/// The side panel's top border: its tab strip.
fn title(workspace: &mut Workspace) -> String {
    left_rows(workspace).swap_remove(0)
}

/// The side panel whose border shows focus, if any.
fn focused_panel(workspace: &mut Workspace) -> Option<char> {
    left_border_focused(workspace, 0).then(|| current_tab(workspace))
}

#[test]
fn one_side_panel_fills_the_column_and_its_title_lists_both_tabs() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());

    let rows = left_rows(&mut workspace);
    assert!(rows[0].contains("─Files─Local branches─"), "{}", rows[0]);
    assert!(rows.last().unwrap().starts_with('╰'), "{}", rows.join("\n"));
    assert_eq!(rows.iter().filter(|row| row.starts_with('╭')).count(), 1);
    assert_eq!(current_tab(&mut workspace), '1');
    let files = left_text(&mut workspace);
    assert!(files.contains("greet.rs"), "{files}");
    assert!(!files.contains("feature"), "{files}");

    show(&mut workspace, '2');
    let branches = panel(&mut workspace, '2');
    assert!(branches.contains("feature"), "{branches}");
    assert!(!branches.contains("greet.rs"), "{branches}");
    let right = right_text(&mut workspace);
    assert!(
        right.lines().next().unwrap().starts_with("╭─Log─"),
        "{right}"
    );
}

#[test]
fn clicking_a_tab_name_switches_the_side_panel() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    focus_main(&mut workspace);

    let branches = tab_column(&mut workspace, '2').unwrap();
    click(&mut workspace, branches + 5, 0);
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("shout greeting"));

    let files = tab_column(&mut workspace, '1').unwrap();
    click(&mut workspace, files, 0);
    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(right_text(&mut workspace).contains("Hi"));
}

#[test]
fn switching_tabs_keeps_each_panel_selection() {
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
fn a_clean_tree_hides_files_and_branches_fills_the_column() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    assert!(
        !has_panel(&mut workspace, '1'),
        "{}",
        left_text(&mut workspace)
    );
    assert!(left_rows(&mut workspace)[0].contains("─Local branches─"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("say hi"));
}

#[test]
fn on_a_clean_tree_focus_stays_on_branches() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    for key in [KeyCode::Tab, KeyCode::BackTab] {
        press(&mut workspace, &[key]);
        assert_eq!(focused_panel(&mut workspace), Some('2'), "{key:?}");
    }
}

#[test]
fn committing_every_change_hides_files_and_focuses_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    press(&mut workspace, &[KeyCode::Enter]);
    assert!(right_focused(&mut workspace));

    let repo = Repository::open(dir.path()).unwrap();
    commit(dir.path(), &repo, &[], "say hi", 400);
    reload(&mut workspace);

    assert!(!has_panel(&mut workspace, '1'));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(right_text(&mut workspace).contains("say hi"));
}

#[test]
fn a_new_change_on_a_clean_tree_focuses_files() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert_eq!(focused_panel(&mut workspace), Some('1'));
    assert!(panel(&mut workspace, '1').contains("greet.rs"));
}

#[test]
fn a_new_change_moves_focus_from_the_branches_main_view_to_files() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());
    focus_main(&mut workspace);

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert!(!right_focused(&mut workspace));
    assert_eq!(focused_panel(&mut workspace), Some('1'));
}

#[test]
fn further_changes_leave_focus_on_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');

    edit_unstaged(dir.path());
    reload(&mut workspace);

    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[test]
fn a_review_on_a_clean_tree_shows_files_until_esc() {
    let dir = clean_fixture();
    let mut workspace = workspace(dir.path());

    press(&mut workspace, &[KeyCode::Enter, KeyCode::Enter]);
    assert!(panel(&mut workspace, '1').contains("Commit "));
    assert_eq!(focused_panel(&mut workspace), Some('1'));

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(!has_panel(&mut workspace, '1'));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[test]
fn the_startup_placeholder_still_opens_on_files_once_built() {
    let dir = fixture();
    let theme = Theme::default();
    let mut workspace = Workspace::new(FilesMode::empty(&theme, 60));
    assert_eq!(focused_panel(&mut workspace), Some('2'));

    workspace.adopt_files(FilesMode::discover_at(dir.path(), &theme, 60));

    assert_eq!(focused_panel(&mut workspace), Some('1'));
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
        .files
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
    assert!(title(&mut workspace).contains("─Files─"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(panel(&mut workspace, '2').contains("Commits (feature)"));
    press(&mut workspace, &[KeyCode::Esc]);
    assert!(panel(&mut workspace, '2').contains("Local branches"));
    // The branch selection survived the round trip.
    assert!(right_text(&mut workspace).contains("add feature"));
    press(&mut workspace, &[KeyCode::Esc]);
    assert!(panel(&mut workspace, '2').contains("Local branches"));
}

/// [`fixture`] with one more commit on `main` whose message has a
/// subject and a body, so its review has details to show.
fn explained_fixture(body: &str) -> tempfile::TempDir {
    let dir = fixture();
    let repo = Repository::open(dir.path()).unwrap();
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
    let repo = Repository::open(dir).unwrap();
    let id = repo.revparse_single("HEAD").unwrap().id().to_string();
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
    assert!(files.contains(&format!("─Commit {short}─")), "{files}");
    let rows = panel_lines(&mut workspace, '1');
    assert!(rows[0].ends_with("explain the change"), "{rows:#?}");
    assert!(!rows[0].contains(&short), "{rows:#?}");
    let lines: Vec<&str> = files.lines().collect();
    assert!(
        lines[2].starts_with("├──") && lines[2].ends_with("──┤"),
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
    assert!(!has_panel(&mut workspace, '2'));
    assert!(title(&mut workspace).starts_with(&format!("╭─Commit {short}──")));

    for key in [KeyCode::Tab, KeyCode::BackTab] {
        press(&mut workspace, &[key]);
        assert_eq!(focused_panel(&mut workspace), Some('1'), "{key:?}");
    }

    press(&mut workspace, &[KeyCode::Esc]);
    assert!(!has_panel(&mut workspace, '1'), "the tree is clean");
    assert_eq!(focused_panel(&mut workspace), Some('2'));
    assert!(panel(&mut workspace, '2').contains("Commits (main)"));
    let preview = right_text(&mut workspace);
    assert!(preview.contains("explain the change"), "{preview}");
}

fn feature_tip(dir: &Path) -> String {
    let repo = Repository::open(dir).unwrap();
    let id = repo.revparse_single("feature").unwrap().id().to_string();
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
    assert!(title(&mut workspace).contains("─Files─"));
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

    assert!(!has_panel(&mut workspace, '1'));
    assert!(panel(&mut workspace, '2').contains("Not a git repository"));
}

#[test]
fn a_reload_picks_up_new_commits_and_branches() {
    let dir = fixture();
    let mut workspace = workspace(dir.path());
    show(&mut workspace, '2');
    assert!(!right_text(&mut workspace).contains("say hi"));
    let repo = Repository::open(dir.path()).unwrap();
    commit(dir.path(), &repo, &[], "say hi", 400);
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("fresh", &head, false).unwrap();

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

/// The inner rows of panel `[n]`, borders and trailing blanks removed.
fn panel_lines(workspace: &mut Workspace, n: char) -> Vec<String> {
    let text = panel(workspace, n);
    let rows: Vec<&str> = text.lines().collect();
    let mut lines: Vec<String> = rows[1..rows.len() - 1]
        .iter()
        .map(|row| {
            let chars: Vec<char> = row.chars().collect();
            chars[1..chars.len() - 1]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string()
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

    let shown = panel_lines(&mut workspace, '2');

    assert!(panel(&mut workspace, '2').contains("Workspace"));
    assert!(!panel(&mut workspace, '2').contains("uncommitted"));
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
            "┴ 3917b63 (common base) 2026-10-14 init",
            "",
            "unapplied",
            "   [parked]",
        ])
        .map(fit)
        .collect();
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
    press(
        &mut workspace,
        &[KeyCode::Char('k'), KeyCode::Char('j'), KeyCode::Enter],
    );

    assert!(panel(&mut workspace, '1').contains("Review top-feature"));
}

/// The fixture repo in a GitButler workspace with `feature` applied:
/// `but status` lists its one commit, by its real id.
#[cfg(unix)]
fn feature_status(dir: &Path) -> std::path::PathBuf {
    let repo = Repository::open(dir).unwrap();
    let feature = repo.revparse_single("feature").unwrap().id().to_string();
    let branch = status_branch("fe", "feature", &[("abcd", &feature, "add feature")]);
    write_status(dir, &[&[branch]])
}

/// Write `but status` JSON into the fixture repo's `.git`: `stacks` of
/// [`status_branch`]es, top first, all resting on `feature`'s parent.
#[cfg(unix)]
fn write_status(dir: &Path, stacks: &[&[String]]) -> std::path::PathBuf {
    let repo = Repository::open(dir).unwrap();
    let base = repo.revparse_single("feature~1").unwrap().id();
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
    assert!(title(&mut workspace).contains("─Files─"));
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
    assert!(title(&mut workspace).contains("─Files─"));
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
    assert!(!has_panel(&mut workspace, '2'));
}

/// Commit `files` on top of `branch` without touching the working tree,
/// and move `branch` to the new commit.
#[cfg(unix)]
fn commit_on(repo: &Repository, branch: &str, files: &[(&str, &str)], msg: &str) -> String {
    let parent = repo
        .revparse_single(branch)
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let mut builder = repo.treebuilder(Some(&parent.tree().unwrap())).unwrap();
    for (path, text) in files {
        let blob = repo.blob(text.as_bytes()).unwrap();
        builder.insert(path, blob, 0o100_644).unwrap();
    }
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let sig = Signature::new("Ada Lovelace", "ada@example.com", &Time::new(400, 0)).unwrap();
    let id = repo
        .commit(None, &sig, &sig, msg, &tree, &[&parent])
        .unwrap();
    repo.reference(&format!("refs/heads/{branch}"), id, true, "test")
        .unwrap();
    id.to_string()
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
    let repo = Repository::open(dir).unwrap();
    let feature = repo.revparse_single("feature").unwrap().id().to_string();
    let tip = repo.find_commit(feature.parse().unwrap()).unwrap();
    repo.branch("upper", &tip, false).unwrap();
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
    assert!(title(&mut workspace).contains("─Files─"));
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

    assert!(title(&mut workspace).contains("─Files─"));
    assert_eq!(focused_panel(&mut workspace), Some('2'));
}

#[cfg(unix)]
#[test]
fn enter_on_an_unapplied_branch_lists_its_commits() {
    let dir = fixture();
    let stubs = tempfile::tempdir().unwrap();
    let mut workspace = stacked_workspace(dir.path(), stubs.path());

    press(&mut workspace, &[KeyCode::Char('G'), KeyCode::Enter]);

    assert!(panel(&mut workspace, '2').contains("─Commits (parked)─"));
    assert!(title(&mut workspace).contains("─Files─"));
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
