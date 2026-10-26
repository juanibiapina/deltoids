//! Go to definition, callers, and hover info from the diff, through
//! `FilesMode`'s keys, mouse, and screen, against a scripted language
//! server.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use lsp_server::{Message, Notification};
use ratatui::style::Modifier;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::model::ResolvedFile;
use super::navigate::Navigation;
use super::test_support::*;
use super::*;
use crate::cli::browse::comments::LineSide;
use crate::cli::browse::test_screen::{self, LEFT, PANE_HEIGHT};
use crate::lsp::fake::{self, Record, Reply, Script};

const MAIN_BEFORE: &str = "mod cart;

fn main() {
    let cart = cart::Cart::default();
}
";

/// Line 5 calls `savings`.
const MAIN_AFTER: &str = "mod cart;

fn main() {
    let cart = cart::Cart::default();
    println!(\"{}\", cart.savings());
}
";

/// `total` (line 4) is unchanged and `len` calls it on unchanged line 9;
/// `savings` (line 12) changes on line 13.
const CART_BEFORE: &str = "pub struct Cart;

impl Cart {
    pub fn total(&self) -> u32 {
        1
    }

    pub fn len(&self) -> u32 {
        self.total()
    }

    pub fn savings(&self) -> u32 {
        0
    }
}
";

const CART_AFTER: &str = "pub struct Cart;

impl Cart {
    pub fn total(&self) -> u32 {
        1
    }

    pub fn len(&self) -> u32 {
        self.total()
    }

    pub fn savings(&self) -> u32 {
        self.total() / 10
    }
}
";

/// Not part of the diff; line 4 calls `total`.
const REPORT: &str = "use crate::cart::Cart;

pub fn summary(cart: &Cart) -> u32 {
    cart.total()
}
";

struct Fixture {
    dir: TempDir,
    state: FilesMode,
    record: Record,
}

/// `src/main.rs` and `src/cart.rs` changed, `src/report.rs` on disk only,
/// and a language server answering with `script(fixture uris)`.
fn fixture(script: impl Fn(&dyn Fn(&str) -> String) -> Script) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("src/main.rs", MAIN_AFTER),
        ("src/cart.rs", CART_AFTER),
        ("src/report.rs", REPORT),
    ] {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    let root = dir.path().to_path_buf();
    let uri = move |path: &str| {
        url::Url::from_file_path(root.join(path))
            .unwrap()
            .to_string()
    };
    let (lsp, record) = fake::lsp(
        dir.path(),
        json!({ "positionEncoding": "utf-8", "callHierarchyProvider": true }),
        script(&uri),
    );
    let mut state = make_state(&[
        changed("src/main.rs", MAIN_BEFORE, MAIN_AFTER),
        changed("src/cart.rs", CART_BEFORE, CART_AFTER),
    ]);
    state.nav = Navigation::new(Some(lsp), Some(dir.path().to_path_buf()));
    Fixture { dir, state, record }
}

fn changed(path: &str, before: &str, after: &str) -> ResolvedFile {
    ResolvedFile {
        file: file_diff(path),
        before: before.to_string(),
        after: after.to_string(),
    }
}

fn location(uri: String, line: u32) -> Value {
    json!({
        "uri": uri,
        "range": { "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 9 } }
    })
}

/// Definitions: `savings` asked from `main.rs` is on `cart.rs:12`,
/// `total` asked from `cart.rs` on `cart.rs:4`. Callers of `total`:
/// `savings` (on a changed line), `len` (on an unchanged line of the
/// changed file), and `summary` (in a file the diff does not touch).
fn shop_server(uri: &dyn Fn(&str) -> String) -> Script {
    let cart = uri("src/cart.rs");
    let report = uri("src/report.rs");
    let item = json!({
        "name": "total", "kind": 6, "uri": cart,
        "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 5, "character": 5 } },
        "selectionRange": { "start": { "line": 3, "character": 11 }, "end": { "line": 3, "character": 16 } }
    });
    let call = |name: &str, uri: &str, line: u32| {
        let mut from = item.clone();
        from["name"] = json!(name);
        from["uri"] = json!(uri);
        json!({ "from": from, "fromRanges": [
            { "start": { "line": line, "character": 8 }, "end": { "line": line, "character": 13 } }
        ] })
    };
    let calls = json!([
        call("len", &cart, 8),
        call("savings", &cart, 12),
        call("summary", &report, 3)
    ]);
    Arc::new(move |message| match message {
        Message::Request(request) if request.method == "textDocument/definition" => {
            let asked = request.params["textDocument"]["uri"]
                .as_str()
                .unwrap_or_default();
            let from = request.params["position"]["line"].as_u64();
            let line = if asked.ends_with("main.rs") || from == Some(11) {
                11
            } else {
                3
            };
            Reply::Result(location(cart.clone(), line))
        }
        Message::Request(request) if request.method == "textDocument/prepareCallHierarchy" => {
            Reply::Result(json!([item]))
        }
        Message::Request(request) if request.method == "callHierarchy/incomingCalls" => {
            Reply::Result(calls.clone())
        }
        _ => Reply::Default,
    })
}

/// Show `path`'s diff with the diff focused.
fn open(state: &mut FilesMode, path: &str) {
    let index = state.model.index_of(path).unwrap();
    state.sidebar.select_file_index(index, PANE_HEIGHT);
    state.snap_diff_to_selected_file();
    state.focus = Focus::Diff;
    test_screen::screen(state);
}

/// Walk the cursor to `path`'s `line` on `side`.
fn cursor_to(state: &mut FilesMode, path: &str, side: LineSide, line: usize) {
    for _ in 0..40 {
        Mode::handle_key(state, KeyCode::Char('k'), PANE_HEIGHT);
    }
    for _ in 0..40 {
        if cursor(state) == Some((path.to_string(), side, line)) {
            return;
        }
        Mode::handle_key(state, KeyCode::Char('j'), PANE_HEIGHT);
    }
    panic!("never reached {path}:{line}; at {:?}", cursor(state));
}

fn cursor(state: &FilesMode) -> Option<(String, LineSide, usize)> {
    state
        .pane(state.shown())
        .cursor_anchor()
        .map(|anchor| (anchor.path.clone(), anchor.side, anchor.line))
}

fn press(state: &mut FilesMode, key: KeyCode) {
    Mode::handle_key(state, key, PANE_HEIGHT);
    test_screen::screen(state);
}

fn ctrl_o(state: &mut FilesMode) {
    Mode::handle_key_event(
        state,
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        PANE_HEIGHT,
    );
    test_screen::screen(state);
}

/// The text drawn underlined on screen.
fn underlined(state: &mut FilesMode) -> String {
    test_screen::screen(state);
    let buffer = test_screen::draw(state);
    let area = buffer.area;
    (area.top()..area.bottom())
        .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
        .map(|position| &buffer[position])
        .filter(|cell| cell.modifier.contains(Modifier::UNDERLINED))
        .map(|cell| cell.symbol().to_string())
        .collect()
}

fn right(state: &mut FilesMode) -> String {
    test_screen::right_text(state)
}

/// Screen row and column where `text` is drawn in the right pane.
fn spot(state: &mut FilesMode, text: &str) -> (u16, u16) {
    let rows = test_screen::screen(state);
    for (y, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        let right: String = chars[LEFT as usize..].iter().collect();
        if let Some(byte) = right.find(text) {
            let column = right[..byte].chars().count() as u16 + LEFT;
            return (y as u16, column);
        }
    }
    panic!("{text:?} is not on screen");
}

fn click(state: &mut FilesMode, button: MouseButton, (row, column): (u16, u16)) {
    Mode::handle_mouse(
        state,
        make_mouse(MouseEventKind::Down(button), column, row),
        PANE_HEIGHT,
    );
}

#[test]
fn enter_on_a_call_jumps_to_its_definition_in_another_file_and_ctrl_o_returns() {
    let Fixture {
        mut state,
        record,
        dir: _dir,
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/main.rs");
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    assert_eq!(underlined(&mut state), "savings", "the call is picked");

    press(&mut state, KeyCode::Enter);

    let asked = &record.params("textDocument/definition")[0];
    assert_eq!(asked["position"], json!({ "line": 4, "character": 24 }));
    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 12))
    );
    assert_eq!(
        selected_path(&state, &state.model).as_deref(),
        Some("src/cart.rs")
    );
    assert!(right(&mut state).contains("src/cart.rs:12  ·  Ctrl+o to go back"));

    ctrl_o(&mut state);
    assert_eq!(
        cursor(&state),
        Some(("src/main.rs".to_string(), LineSide::New, 5))
    );
}

#[test]
fn enter_on_a_definition_lists_its_callers_instead_of_jumping_to_itself() {
    let Fixture {
        mut state,
        record,
        dir: _dir,
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/cart.rs");
    cursor_to(&mut state, "src/cart.rs", LineSide::New, 12);
    assert_eq!(underlined(&mut state), "savings");

    press(&mut state, KeyCode::Enter);

    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 12))
    );
    assert_eq!(record.params("textDocument/prepareCallHierarchy").len(), 1);
    let screen = test_screen::screen(&mut state).join("\n");
    assert!(screen.contains("Callers of savings"), "{screen}");
}

#[test]
fn a_version_older_than_the_disk_navigates_through_lines_that_did_not_change() {
    let script = |uri: &dyn Fn(&str) -> String| -> Script {
        let cart = uri("src/cart.rs");
        Arc::new(move |message| match message {
            Message::Request(request) if request.method == "textDocument/definition" => {
                let from = request.params["position"]["line"].as_u64();
                // `savings` is two lines further down on disk; asked from
                // `main.rs` (shown line 5, disk line 7) it answers its disk
                // line 14, and anything else lands on a line added since.
                let line = if from == Some(6) { 13 } else { 0 };
                Reply::Result(location(cart.clone(), line))
            }
            _ => Reply::Default,
        })
    };
    let Fixture {
        mut state,
        record,
        dir,
    } = fixture(script);
    for (path, text) in [("src/main.rs", MAIN_AFTER), ("src/cart.rs", CART_AFTER)] {
        fs::write(dir.path().join(path), format!("// added\n// since\n{text}")).unwrap();
    }
    open(&mut state, "src/main.rs");
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    assert_eq!(underlined(&mut state), "savings");

    press(&mut state, KeyCode::Enter);
    let asked = &record.params("textDocument/definition")[0];
    assert_eq!(
        asked["position"]["line"],
        json!(6),
        "asked at the disk line"
    );
    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 12)),
        "landed on the shown line"
    );

    cursor_to(&mut state, "src/cart.rs", LineSide::New, 13);
    press(&mut state, KeyCode::Enter);
    assert!(right(&mut state).contains("src/cart.rs:1 has changed since this version"));
}

#[test]
fn h_l_and_arrows_move_the_underline_along_the_line() {
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/main.rs");
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    press(&mut state, KeyCode::Char('h'));
    assert_eq!(underlined(&mut state), "cart");
    press(&mut state, KeyCode::Char('h'));
    assert_eq!(underlined(&mut state), "println");
    press(&mut state, KeyCode::Char('l'));
    assert_eq!(underlined(&mut state), "cart");
    press(&mut state, KeyCode::Left);
    assert_eq!(underlined(&mut state), "println");
    press(&mut state, KeyCode::Right);
    assert_eq!(underlined(&mut state), "cart");
}

#[test]
fn a_definition_outside_the_hunks_opens_the_hunk_and_x_folds_it() {
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/cart.rs");
    cursor_to(&mut state, "src/cart.rs", LineSide::New, 13);
    assert_eq!(underlined(&mut state), "total");
    let total = CommentAnchor {
        path: "src/cart.rs".to_string(),
        side: LineSide::New,
        line: 4,
    };
    assert!(state.model.line(&total).is_none(), "total starts hidden");

    press(&mut state, KeyCode::Enter);
    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 4))
    );
    assert!(right(&mut state).contains("opened like z, x folds it"));

    press(&mut state, KeyCode::Char('x'));
    assert!(state.model.line(&total).is_none(), "x folds it away again");
}

#[test]
fn callers_list_the_diff_first_and_an_outside_caller_opens_as_a_visited_file() {
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/cart.rs");
    cursor_to(&mut state, "src/cart.rs", LineSide::New, 13);

    press(&mut state, KeyCode::Char('r'));
    let screen = test_screen::screen(&mut state).join("\n");
    let inside = screen.find("In this diff").expect("in-diff group");
    let outside = screen.find("Outside the diff").expect("outside group");
    assert!(screen.contains("Callers of total"));
    assert!(inside < screen.find("src/cart.rs:13").unwrap());
    assert!(screen.find("src/cart.rs:13").unwrap() < outside);
    assert!(
        outside < screen.find("src/cart.rs:9").unwrap(),
        "an unchanged line of a changed file is outside the diff: {screen}"
    );
    assert!(outside < screen.find("src/report.rs:4").unwrap());

    press(&mut state, KeyCode::Char('j'));
    press(&mut state, KeyCode::Char('j'));
    press(&mut state, KeyCode::Enter);
    assert_eq!(
        cursor(&state),
        Some(("src/report.rs".to_string(), LineSide::New, 4))
    );
    let screen = test_screen::screen(&mut state).join("\n");
    let sidebar = test_screen::left_text(&mut state);
    let row = sidebar
        .lines()
        .find(|row| row.contains("report.rs"))
        .unwrap_or_default();
    assert!(row.contains('○') && !row.contains("visited"), "{row}");
    assert!(screen.contains("─○ src/report.rs "), "{screen}");
    assert!(screen.contains("not in this diff"), "{screen}");
    assert_eq!(
        state.sidebar.totals().files,
        2,
        "a visited file is not a change"
    );

    ctrl_o(&mut state);
    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 13))
    );
    assert!(
        !test_screen::left_text(&mut state).contains("report.rs"),
        "going back past a visited file drops it"
    );
}

#[test]
fn visited_files_stay_through_edits_and_go_when_the_diff_holds_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    for (path, text) in [
        ("src/main.rs", MAIN_BEFORE),
        ("src/cart.rs", CART_BEFORE),
        ("src/report.rs", REPORT),
    ] {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    stage_all(&repo);
    commit_index(&repo, "initial");
    fs::write(dir.path().join("src/main.rs"), MAIN_AFTER).unwrap();
    fs::write(dir.path().join("src/cart.rs"), CART_AFTER).unwrap();

    store_blobs(dir.path());
    let vcs = Vcs::discover_at(dir.path()).unwrap();
    let (input, model) = FilesMode::try_model(&vcs).unwrap();
    let mut state = FilesMode::new(model, input, Some(vcs), &theme(), 80);
    let root = dir.path().to_path_buf();
    let uri = move |path: &str| {
        url::Url::from_file_path(root.join(path))
            .unwrap()
            .to_string()
    };
    let (lsp, _) = fake::lsp(
        dir.path(),
        json!({ "positionEncoding": "utf-8", "callHierarchyProvider": true }),
        shop_server(&uri),
    );
    state.nav = Navigation::new(Some(lsp), Some(dir.path().to_path_buf()));
    let reload = |state: &mut FilesMode| {
        store_blobs(dir.path());
        let viewport = Viewport {
            height: PANE_HEIGHT,
            diff_width: 80,
        };
        Mode::reload(state, viewport, &theme()).unwrap();
        test_screen::screen(state);
    };
    let sidebar_has_report =
        |state: &mut FilesMode| test_screen::left_text(state).contains("report.rs");

    open(&mut state, "src/cart.rs");
    cursor_to(&mut state, "src/cart.rs", LineSide::New, 13);
    press(&mut state, KeyCode::Char('r'));
    press(&mut state, KeyCode::Char('j'));
    press(&mut state, KeyCode::Char('j'));
    press(&mut state, KeyCode::Enter);
    assert!(sidebar_has_report(&mut state));

    fs::write(
        dir.path().join("src/cart.rs"),
        CART_AFTER.replace("/ 10", "/ 20"),
    )
    .unwrap();
    reload(&mut state);
    assert!(sidebar_has_report(&mut state), "an edit keeps it");

    git_cli(dir.path(), &["add", "src/main.rs"]);
    git_cli(dir.path(), &["commit", "-qm", "main"]);
    reload(&mut state);
    assert!(!sidebar_has_report(&mut state), "a commit drops it");
    ctrl_o(&mut state);
    assert!(right(&mut state).contains("No earlier place to go back to"));
}

#[test]
fn removed_lines_and_files_changed_on_disk_say_why_they_cannot_be_asked_about() {
    let Fixture {
        mut state,
        record,
        dir,
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/cart.rs");
    cursor_to(&mut state, "src/cart.rs", LineSide::New, 13);
    Mode::handle_key(&mut state, KeyCode::Char('k'), PANE_HEIGHT);
    let removed = cursor(&state).unwrap();
    assert_eq!(
        removed.1,
        LineSide::Old,
        "the line above the added one is removed"
    );
    press(&mut state, KeyCode::Enter);
    assert!(right(&mut state).contains("Removed lines aren't on disk; pick a line that is"));

    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    open(&mut state, "src/main.rs");
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    press(&mut state, KeyCode::Enter);
    assert!(right(&mut state).contains("This line has changed since this version"));
    assert!(record.params("textDocument/definition").is_empty());
}

#[test]
fn a_click_picks_a_symbol_a_double_click_goes_to_it_and_a_right_click_lists_callers() {
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/main.rs");
    let at_cart = spot(&mut state, "cart.savings");
    click(&mut state, MouseButton::Left, at_cart);
    assert_eq!(
        cursor(&state),
        Some(("src/main.rs".to_string(), LineSide::New, 5))
    );
    assert_eq!(underlined(&mut state), "cart");

    std::thread::sleep(Duration::from_millis(600));
    let (row, column) = spot(&mut state, "savings()");
    click(&mut state, MouseButton::Left, (row, column));
    click(&mut state, MouseButton::Left, (row, column));
    test_screen::screen(&mut state);
    assert_eq!(
        cursor(&state),
        Some(("src/cart.rs".to_string(), LineSide::New, 12))
    );

    let at_total = spot(&mut state, "total() / 10");
    click(&mut state, MouseButton::Right, at_total);
    let screen = test_screen::screen(&mut state).join("\n");
    assert!(screen.contains("Callers of total"), "{screen}");
}

/// The shop server, also answering hover with `info` as Markdown.
fn hovering(info: &'static str) -> impl Fn(&dyn Fn(&str) -> String) -> Script {
    move |uri| {
        let shop = shop_server(uri);
        Arc::new(move |message| match message {
            Message::Request(request) if request.method == "textDocument/hover" => {
                Reply::Result(json!({ "contents": { "kind": "markdown", "value": info } }))
            }
            message => shop(message),
        })
    }
}

const SAVINGS_INFO: &str =
    "```rust\npub fn savings(&self) -> u32\n```\n\n---\n\nWhat the cart saves.";

/// Move the pointer to `(row, column)`.
fn point(state: &mut FilesMode, (row, column): (u16, u16)) {
    Mode::handle_mouse(
        state,
        make_mouse(MouseEventKind::Moved, column, row),
        PANE_HEIGHT,
    );
}

fn wheel(state: &mut FilesMode, kind: MouseEventKind, (row, column): (u16, u16)) {
    Mode::handle_mouse(state, make_mouse(kind, column, row), PANE_HEIGHT);
}

fn on_screen(state: &mut FilesMode, text: &str) -> bool {
    test_screen::screen(state).join("\n").contains(text)
}

#[test]
fn resting_the_pointer_on_a_symbol_shows_its_info_until_it_moves_off() {
    let Fixture {
        mut state,
        record,
        dir: _dir,
    } = fixture(hovering(SAVINGS_INFO));
    open(&mut state, "src/main.rs");
    let cursor_before = cursor(&state);
    let at = spot(&mut state, "savings()");

    point(&mut state, at);
    let below = spot(&mut state, "pub fn savings(&self) -> u32");
    assert_eq!(
        below.0,
        at.0 + 2,
        "the popup's first row sits under the symbol"
    );
    assert!(on_screen(&mut state, "What the cart saves."));
    let asked = &record.params("textDocument/hover")[0];
    assert_eq!(asked["position"], json!({ "line": 4, "character": 24 }));
    assert_eq!(
        cursor(&state),
        cursor_before,
        "hover leaves the cursor alone"
    );

    point(&mut state, below);
    assert!(
        on_screen(&mut state, "What the cart saves."),
        "the popup stays while the pointer is on it"
    );

    point(&mut state, (at.0, 2));
    assert!(!on_screen(&mut state, "What the cart saves."));
    assert_eq!(record.params("textDocument/hover").len(), 1);
}

#[test]
fn sweeping_across_symbols_asks_only_where_the_pointer_rests() {
    let Fixture {
        mut state,
        record,
        dir: _dir,
    } = fixture(hovering(SAVINGS_INFO));
    open(&mut state, "src/main.rs");
    let (cart, savings) = (
        spot(&mut state, "cart.savings"),
        spot(&mut state, "savings()"),
    );
    point(&mut state, cart);
    point(&mut state, savings);
    assert!(on_screen(&mut state, "What the cart saves."));
    let asked = record.params("textDocument/hover");
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["position"]["character"], json!(24));
}

#[test]
fn i_shows_info_for_the_picked_symbol_and_any_key_closes_it() {
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(hovering(SAVINGS_INFO));
    open(&mut state, "src/main.rs");
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);

    press(&mut state, KeyCode::Char('i'));
    assert!(on_screen(&mut state, "What the cart saves."));
    assert!(navigate::escape(&mut state), "Esc closes the popup");
    assert!(!on_screen(&mut state, "What the cart saves."));
    assert_eq!(
        cursor(&state),
        Some(("src/main.rs".to_string(), LineSide::New, 5))
    );

    press(&mut state, KeyCode::Char('i'));
    assert!(on_screen(&mut state, "What the cart saves."));
    press(&mut state, KeyCode::Char('k'));
    assert!(!on_screen(&mut state, "What the cart saves."));
    assert_eq!(
        cursor(&state),
        Some(("src/main.rs".to_string(), LineSide::New, 4)),
        "the key that closes the popup still moves the cursor"
    );
}

#[test]
fn hover_stays_quiet_where_i_says_why_there_is_nothing() {
    let Fixture {
        mut state,
        record,
        dir,
    } = fixture(|uri| shop_server(uri));
    open(&mut state, "src/main.rs");
    let at = spot(&mut state, "savings()");
    point(&mut state, at);
    assert!(
        !on_screen(&mut state, "No info found"),
        "the pointer never sets the status"
    );
    assert_eq!(record.params("textDocument/hover").len(), 1);

    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    press(&mut state, KeyCode::Char('i'));
    assert!(on_screen(&mut state, "No info found"));

    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    press(&mut state, KeyCode::Char('j'));
    point(&mut state, (at.0, 2));
    point(&mut state, at);
    test_screen::screen(&mut state);
    assert_eq!(
        record.params("textDocument/hover").len(),
        2,
        "a line that changed on disk is never asked about"
    );
    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    press(&mut state, KeyCode::Char('i'));
    assert!(on_screen(
        &mut state,
        "This line has changed since this version"
    ));
}

#[test]
fn the_wheel_scrolls_a_long_hover_under_the_pointer() {
    let long: &'static str = (1..=40)
        .map(|row| format!("row {row:02}"))
        .collect::<Vec<_>>()
        .join("\n\n")
        .leak();
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(hovering(long));
    open(&mut state, "src/main.rs");
    let at = spot(&mut state, "savings()");
    point(&mut state, at);
    assert!(on_screen(&mut state, "row 01"));
    assert!(on_screen(&mut state, "↓ more"));

    let popup = spot(&mut state, "row 02");
    for _ in 0..4 {
        wheel(&mut state, MouseEventKind::ScrollDown, popup);
    }
    assert!(!on_screen(&mut state, "row 01"), "the popup scrolled");
    assert!(on_screen(&mut state, "row 03"));

    wheel(&mut state, MouseEventKind::ScrollDown, (at.0, 2));
    assert!(
        !on_screen(&mut state, "row 03"),
        "the wheel elsewhere closes it"
    );
}

#[test]
fn the_border_shows_indexing_progress_and_the_footer_the_lookup() {
    let script = |_: &dyn Fn(&str) -> String| -> Script {
        let progress = |value: Value| {
            Message::Notification(Notification::new(
                "$/progress".to_string(),
                json!({ "token": "index", "value": value }),
            ))
        };
        Arc::new(move |message| match message {
            Message::Notification(note) if note.method == "initialized" => Reply::Then(
                vec![progress(
                    json!({ "kind": "begin", "title": "Indexing", "percentage": 62 }),
                )],
                None,
            ),
            Message::Request(request) if request.method == "textDocument/definition" => {
                Reply::Silent
            }
            Message::Notification(note) if note.method == "$/cancelRequest" => {
                Reply::Then(vec![progress(json!({ "kind": "end" }))], None)
            }
            _ => Reply::Default,
        })
    };
    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(script);
    open_without_settling(&mut state, "src/main.rs");
    assert!(
        eventually(|| {
            let border = border(&mut state);
            border.contains(" Rust ") && border.contains("62%")
        }),
        "the border shows indexing progress"
    );
    assert!(
        Mode::background(&mut state, true).pending,
        "the spinner ticks"
    );

    cursor_to(&mut state, "src/main.rs", LineSide::New, 5);
    Mode::handle_key(&mut state, KeyCode::Enter, PANE_HEIGHT);
    assert!(eventually(|| {
        let screen = text(&test_screen::draw(&mut state));
        Mode::background(&mut state, true);
        screen.contains("Finding definition of savings")
    }));

    Mode::handle_key(&mut state, KeyCode::Esc, PANE_HEIGHT);
    let mut work = Mode::background(&mut state, true);
    assert!(
        eventually(|| {
            work = Mode::background(&mut state, true);
            !work.pending
        }),
        "an idle server lets the TUI rest"
    );
    assert!(
        work.changed,
        "the tick that ends the work draws the spinner away"
    );
    assert!(
        border(&mut state).contains("Rust ●"),
        "indexing ended: ready"
    );
}

#[test]
fn the_border_shows_the_server_state_before_the_diff_has_focus() {
    let Fixture {
        mut state,
        dir: _dir,
        record,
    } = fixture(|uri| shop_server(uri));
    assert_eq!(state.focus, Focus::Sidebar);
    assert!(eventually(|| border(&mut state).contains("Rust ●")));
    assert_eq!(
        record.launches(),
        1,
        "shown, not focused, is enough to start"
    );

    let Fixture {
        mut state,
        dir: _dir,
        ..
    } = fixture(|_| Arc::new(|_| Reply::Hangup));
    assert!(eventually(|| border(&mut state).contains("Rust ✕")));
}

/// The diff pane's top border, drawing a frame first.
fn border(state: &mut FilesMode) -> String {
    let screen = text(&test_screen::draw(state));
    Mode::background(state, true);
    screen
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .skip(LEFT as usize)
        .collect()
}

/// Store the working tree's changed files as blobs, so content
/// resolution does not depend on the process cwd shared by parallel
/// tests, as the interaction tests do.
fn store_blobs(dir: &std::path::Path) {
    let raw = git2::Repository::open(dir).unwrap();
    let vcs = Vcs::discover_at(dir).unwrap();
    let stages = vcs.repo().working_tree_snapshot().unwrap().stages;
    for bytes in stages
        .into_iter()
        .flat_map(|status| status.paths)
        .filter_map(|path| fs::read(dir.join(path)).ok())
    {
        raw.blob(&bytes).unwrap();
    }
}

/// Show `path`'s diff without waiting for background work to settle,
/// which never happens while a lookup is pending.
fn open_without_settling(state: &mut FilesMode, path: &str) {
    let index = state.model.index_of(path).unwrap();
    state.sidebar.select_file_index(index, PANE_HEIGHT);
    state.snap_diff_to_selected_file();
    state.focus = Focus::Diff;
    for _ in 0..200 {
        test_screen::draw(state);
        let work = Mode::background(state, true);
        if !state.unstaged.cache.pending() && !work.changed {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn text(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn eventually(mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}
