//! The definitions view and summaries through keys, mouse, and the drawn
//! screen.

use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::model::ResolvedFile;
use super::test_support::*;
use super::*;
use crate::cli::browse::test_screen::{self, LEFT, PANE_HEIGHT};

const BEFORE: &str = "\
pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn total(&self) -> u32 {
        self.items.iter().sum()
    }

    fn old_name(&self) -> usize {
        let count = self.items.len();
        let doubled = count * 2;
        doubled + count
    }
}
";

const AFTER: &str = "\
pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn total(&self, discount: u32) -> u32 {
        self.items.iter().sum::<u32>() - discount
    }

    fn new_name(&self) -> usize {
        let count = self.items.len();
        let doubled = count * 2;
        doubled + count
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}
";

fn files() -> Vec<ResolvedFile> {
    vec![
        ResolvedFile {
            file: file_diff("src/cart.rs"),
            before: BEFORE.to_string(),
            after: AFTER.to_string(),
        },
        ResolvedFile {
            file: file_diff("src/other.rs"),
            before: "fn one() {\n    1;\n}\n".to_string(),
            after: "fn one() {\n    2;\n}\n".to_string(),
        },
    ]
}

fn on_file(index: usize) -> FilesMode {
    let mut state = make_state(&files());
    state.sidebar.select_file_index(index, 20);
    state.snap_diff_to_selected_file();
    state
}

/// The right pane's rows.
fn pane(state: &mut FilesMode) -> Vec<String> {
    test_screen::right_text(state)
        .lines()
        .map(str::to_string)
        .collect()
}

fn row_with<'a>(rows: &'a [String], text: &str) -> (usize, &'a String) {
    rows.iter()
        .enumerate()
        .find(|(_, row)| row.contains(text))
        .unwrap_or_else(|| panic!("no row with {text:?}: {rows:#?}"))
}

fn press(state: &mut FilesMode, key: KeyCode) {
    Mode::handle_key(state, key, PANE_HEIGHT);
}

#[test]
fn o_switches_a_file_between_its_diff_and_its_definitions() {
    let mut state = on_file(0);
    assert!(
        pane(&mut state)
            .join("\n")
            .contains("self.items.iter().sum()")
    );

    press(&mut state, KeyCode::Char('o'));
    let rows = pane(&mut state);
    let text = rows.join("\n");
    assert!(rows[0].contains("src/cart.rs · definitions"), "{}", rows[0]);
    assert!(text.contains("impl Cart"), "{text}");
    let (total, _) = row_with(&rows, "~ pub fn total(&self, discount: u32) -> u32");
    assert!(
        rows[total + 1].contains("was pub fn total(&self) -> u32"),
        "{text}"
    );
    assert!(
        text.contains("→ fn new_name(&self) -> usize  renamed from"),
        "{text}"
    );
    assert!(text.contains("+ pub fn clear(&mut self)"), "{text}");
    assert!(!text.contains("self.items.iter()"), "no diff lines: {text}");

    state.sidebar.select_file_index(1, 20);
    state.snap_diff_to_selected_file();
    let rows = pane(&mut state);
    assert!(
        rows[0].contains("src/other.rs · definitions"),
        "{}",
        rows[0]
    );

    press(&mut state, KeyCode::Char('o'));
    assert!(pane(&mut state).join("\n").contains("2;"));
}

#[test]
fn clicking_a_definition_opens_the_diff_at_its_change() {
    let mut state = on_file(0);
    press(&mut state, KeyCode::Char('o'));
    let rows = pane(&mut state);
    let (y, _) = row_with(&rows, "+ pub fn clear");

    Mode::handle_mouse(
        &mut state,
        make_mouse(MouseEventKind::Down(MouseButton::Left), LEFT + 6, y as u16),
        PANE_HEIGHT,
    );

    let rows = pane(&mut state);
    assert!(
        rows[0].contains("src/cart.rs") && !rows[0].contains("definitions"),
        "{}",
        rows[0]
    );
    let line = AFTER
        .lines()
        .position(|line| line.contains("pub fn clear"))
        .unwrap()
        + 1;
    let anchor = state.unstaged.cursor_anchor().expect("cursor on a line");
    assert_eq!((anchor.path.as_str(), anchor.line), ("src/cart.rs", line));
}

#[test]
fn a_definitions_signature_is_highlighted() {
    let mut state = on_file(0);
    press(&mut state, KeyCode::Char('o'));
    pane(&mut state);
    let row = state
        .unstaged
        .rows()
        .iter()
        .find(|row| line_text(&row.line).contains("pub fn clear"))
        .expect("a row for clear");
    let colors: std::collections::HashSet<_> =
        row.line.spans.iter().map(|span| span.style.fg).collect();
    assert!(colors.len() > 3, "{:?}", row.line);
}

#[test]
fn o_on_a_directory_shows_every_files_diff_and_back() {
    let mut state = make_state(&files());
    assert!(state.sidebar.select_directory_path("src/", 20));
    state.snap_diff_to_selected_file();
    assert!(pane(&mut state).join("\n").contains("2 files changed"));

    press(&mut state, KeyCode::Char('o'));
    let text = pane(&mut state).join("\n");
    assert!(text.contains("self.items.iter()"), "{text}");
    assert!(!text.contains("files changed"), "{text}");

    press(&mut state, KeyCode::Char('o'));
    assert!(pane(&mut state).join("\n").contains("2 files changed"));
}

#[test]
fn a_block_reads_as_a_placeholder_until_it_is_drawn() {
    let mut state = on_file(0);
    state.file_definitions = true;
    let first = state.visible_diff_window(DrawBudget::Fast);
    let text: Vec<String> = first.iter().map(line_text).collect();
    assert!(
        text.iter().any(|row| row.contains("Reading definitions")),
        "{text:#?}"
    );

    let drawn = state.visible_diff_window(DrawBudget::Full);
    let text: Vec<String> = drawn.iter().map(line_text).collect();
    assert!(
        text.iter().any(|row| row.contains("pub fn clear")),
        "{text:#?}"
    );
}

#[test]
#[ignore = "release performance probe; run with --release --ignored --nocapture"]
fn a_commit_summary_of_many_files_paints_fast_and_fills_in_soon() {
    let files: Vec<ResolvedFile> = (0..100)
        .map(|index| {
            let before: String = (0..60)
                .map(|f| format!("fn f{f}(x: u32) -> u32 {{\n    x + {f}\n}}\n\n"))
                .collect();
            let after = before
                .replace("x + 7\n", "x + 70\n")
                .replace("x + 40\n", "x * 40\n");
            ResolvedFile {
                file: file_diff(&format!("src/f{index:03}.rs")),
                before,
                after,
            }
        })
        .collect();
    let mut state = make_state(&files);
    assert!(state.sidebar.select_directory_path("src/", 20));
    state.snap_diff_to_selected_file();
    let start = std::time::Instant::now();
    state.visible_diff_window(DrawBudget::Fast);
    let first = start.elapsed();
    let start = std::time::Instant::now();
    let rows = state.visible_diff_window(DrawBudget::Full);
    let complete = start.elapsed();
    let text: Vec<String> = rows.iter().map(line_text).collect();
    assert!(!text.iter().any(|row| row.contains("Reading definitions")));
    println!(
        "100-file summary: first frame {first:?}, complete {complete:?}, rows {}",
        rows.len()
    );
    assert!(
        first < std::time::Duration::from_millis(33),
        "first frame {first:?}"
    );
    assert!(
        complete < std::time::Duration::from_secs(1),
        "complete {complete:?}"
    );
}
