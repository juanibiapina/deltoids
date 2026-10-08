use super::model::ResolvedFile;
use super::test_support::*;
use super::*;

fn sidebar_paths(state: &FilesMode) -> Vec<String> {
    state
        .sidebar
        .display_order()
        .iter()
        .map(|&index| display_path(&state.model.files[index].file).to_string())
        .collect()
}

fn sidebar_texts(state: &FilesMode) -> Vec<String> {
    state.sidebar.rows().iter().map(line_text).collect()
}

fn four_files() -> FilesMode {
    make_state(&[
        resolved("CHANGELOG.md"),
        resolved("Cargo.lock"),
        resolved("src/a.rs"),
        resolved("tests/a.rs"),
    ])
}

/// A stand-in for Jev: hunks of `core_path` are critical core code,
/// hunks under `tests/` are routine tests, and every other hunk is a
/// routine refactor.
pub(super) fn fake_jev(core_path: &'static str) -> review::Sender {
    std::sync::Arc::new(move |request: &crate::judgments::JevRequest| {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let mut answers = serde_json::Map::new();
        for (id, hunk) in body["state"]["hunks"].as_object().unwrap() {
            let file = hunk["file"].as_str().unwrap();
            let (role, attention) = if file == core_path {
                ("core", 2.0)
            } else if file.starts_with("tests/") {
                ("test", 1.0)
            } else {
                ("refactor", 1.0)
            };
            answers.insert(
                format!("role_{id}"),
                serde_json::json!({ "choice": role, "confidence": 0.9 }),
            );
            answers.insert(
                format!("attention_{id}"),
                serde_json::json!({ "score": attention }),
            );
            answers.insert(format!("breaking_{id}"), serde_json::json!({ "noul": 0.0 }));
        }
        Ok(serde_json::json!({ "answers": answers }).to_string())
    })
}

fn failing_jev() -> review::Sender {
    std::sync::Arc::new(|_: &crate::judgments::JevRequest| {
        Err(crate::judgments::SendError::Failed(
            "Jev rejected the key in TYPESAFE_API_KEY".to_string(),
        ))
    })
}

pub(super) fn wait_for_review(state: &mut FilesMode) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.review.pending() {
        state.background(true);
        assert!(Instant::now() < deadline, "review job did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn with_jev(files: &[ResolvedFile], sender: review::Sender) -> FilesMode {
    let mut state = make_state(files);
    state.review = review::Review::with_sender(sender);
    state.start_review();
    state
}

#[test]
fn jev_tags_rows_without_moving_the_tree() {
    let files = [resolved("a/x.rs"), resolved("b/y.rs")];
    let tree = sidebar_paths(&make_state(&files));

    let mut state = with_jev(&files, fake_jev("b/y.rs"));
    wait_for_review(&mut state);

    assert_eq!(sidebar_paths(&state), tree);
    assert_eq!(
        sidebar_texts(&state),
        ["▼ a/", "  M x.rs +1 -1", "▼ b/", "  M y.rs +1 -1"]
    );
    let notes: Vec<Option<String>> = state
        .sidebar
        .row_notes()
        .iter()
        .map(|note| note.as_ref().map(line_text))
        .collect();
    assert_eq!(
        notes,
        [
            None,
            Some("refactor ●".to_string()),
            None,
            Some("core ●".to_string())
        ]
    );
}

#[test]
fn without_jev_rows_keep_their_line_counts_and_carry_no_tags() {
    let state = four_files();

    assert_eq!(
        sidebar_texts(&state),
        [
            "M CHANGELOG.md +1 -1",
            "M Cargo.lock +1 -1",
            "▼ src/",
            "  M a.rs +1 -1",
            "▼ tests/",
            "  M a.rs +1 -1"
        ]
    );
    assert!(state.sidebar.row_notes().iter().all(Option::is_none));
}

#[test]
fn a_failed_jev_call_leaves_the_rows_untagged() {
    let mut state = with_jev(
        &[resolved("src/a.rs"), resolved("tests/a.rs")],
        failing_jev(),
    );
    wait_for_review(&mut state);

    assert_eq!(
        state.status.as_deref(),
        Some("Jev unavailable: Jev rejected the key in TYPESAFE_API_KEY")
    );
    assert!(sidebar_texts(&state).contains(&"  M a.rs +1 -1".to_string()));
}

#[test]
fn lockfiles_are_tagged_without_asking_jev() {
    let state = four_files_with_jev();

    let lockfile_row = state
        .sidebar
        .rows()
        .iter()
        .position(|row| line_text(row).contains("Cargo.lock"))
        .unwrap();
    let note = state.sidebar.row_notes()[lockfile_row]
        .as_ref()
        .map(line_text);

    assert_eq!(note.as_deref(), Some("lockfile  "));
}

fn four_files_with_jev() -> FilesMode {
    let mut state = with_jev(
        &[
            resolved("CHANGELOG.md"),
            resolved("Cargo.lock"),
            resolved("src/a.rs"),
            resolved("tests/a.rs"),
        ],
        fake_jev("src/a.rs"),
    );
    wait_for_review(&mut state);
    state
}

#[test]
fn f_hides_low_value_files_and_brings_them_back() {
    let mut state = four_files_with_jev();
    let all = sidebar_paths(&state);

    Mode::handle_key(&mut state, KeyCode::Char('f'), 20);
    assert_eq!(sidebar_paths(&state), ["CHANGELOG.md", "src/a.rs"]);

    Mode::handle_key(&mut state, KeyCode::Char('f'), 20);
    assert_eq!(sidebar_paths(&state), all);
}

#[test]
fn f_hides_nothing_without_jev() {
    let mut state = four_files();
    let all = sidebar_paths(&state);

    Mode::handle_key(&mut state, KeyCode::Char('f'), 20);

    assert_eq!(sidebar_paths(&state), all);
}

fn hunk_header_texts(state: &mut FilesMode) -> Vec<String> {
    state
        .visible_diff_window(DrawBudget::Full)
        .iter()
        .map(line_text)
        .filter(|text| text.contains('│'))
        .collect()
}

#[test]
fn hunk_headers_show_the_role_and_an_attention_dot_at_the_right_edge() {
    let mut state = make_state(&[resolved("src/a.rs")]);
    let before = hunk_header_texts(&mut state);
    state.review = review::Review::with_sender(fake_jev("src/a.rs"));
    state.start_review();
    wait_for_review(&mut state);

    assert!(
        before.iter().all(|text| !text.contains("core")),
        "{before:?}"
    );
    let headers = hunk_header_texts(&mut state);
    let width = state.unstaged.cached_width;
    let header = headers
        .iter()
        .find(|text| text.ends_with(" core ●"))
        .unwrap_or_else(|| panic!("{headers:?}"));
    assert!(header.starts_with("1 │  "), "{header:?}");
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(header.as_str()),
        width
    );
    let spans: Vec<_> = state
        .visible_diff_window(DrawBudget::Full)
        .iter()
        .flat_map(|line| line.spans.clone())
        .collect();
    let tag = spans.iter().find(|span| span.content == "core").unwrap();
    assert_eq!(tag.style.fg, None);
    let dot = spans.iter().find(|span| span.content == "●").unwrap();
    assert_eq!(
        dot.style.fg,
        Some(deltoids::render_tui::rgb_to_color(theme().status_deleted))
    );
}
