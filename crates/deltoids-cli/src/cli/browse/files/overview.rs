//! Summaries: what the diff pane shows for a directory, or for a whole
//! commit when its root row is selected. A line of totals, then each file
//! of the shown column: a header (its status, path, review note, comment
//! count, and line counts) and its block of changed definitions (see
//! [`super::definitions_view`]). Blocks come from the pane's definitions
//! cache, drawn in the background; a file whose block is not ready shows
//! one placeholder row. Opening a header shows the file's diff.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use crate::cli::browse::comments::CommentStore;
use crate::cli::browse::diff_cursor::{DiffRow, LinePlace, Opens};
use crate::cli::browse::text::{display_width, shorten_right};
use crate::sidebar::{FileStatus, RowNote, display_path, file_status, render_note, status_color};

use super::model::{Column, Model, body_deltas};

/// What a summary covers: files of the shown column in sidebar order,
/// shown with paths inside `directory` (`src/cli/`, or empty for a whole
/// commit), each file's review note by model index, and the session's
/// comments.
pub(super) struct Selection<'a> {
    pub(super) directory: &'a str,
    pub(super) column: Column,
    pub(super) files: &'a [usize],
    pub(super) notes: &'a [Option<RowNote>],
    pub(super) comments: &'a CommentStore,
}

/// The summary's rows for a pane `width` columns wide. `blocks[i]` is the
/// rendered block of `selection.files[i]`, when it is ready.
pub(super) fn rows(
    model: &Model,
    selection: &Selection<'_>,
    blocks: &[Option<&[DiffRow]>],
    width: usize,
    theme: &Theme,
) -> Vec<DiffRow> {
    let entries: Vec<Entry> = selection
        .files
        .iter()
        .map(|&index| Entry::of(model, selection, index))
        .collect();
    let mut rows = vec![DiffRow::plain(totals(&entries, theme))];
    for (entry, block) in entries.iter().zip(blocks) {
        rows.push(DiffRow::plain(Line::default()));
        rows.push(entry.header(width, theme));
        match block {
            Some(block) => rows.extend(block.iter().cloned()),
            None => rows.push(placeholder(theme)),
        }
    }
    rows
}

/// The row a block shows until it is drawn.
pub(super) fn placeholder(theme: &Theme) -> DiffRow {
    DiffRow::plain(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Reading definitions\u{2026}",
            Style::default().fg(rgb_to_color(theme.muted)),
        ),
    ]))
}

/// One file's facts, gathered before layout.
struct Entry {
    /// The file's path, the cursor's and the diff's name for it.
    file: String,
    /// `None` for a file opened by a jump.
    status: Option<FileStatus>,
    /// What the header shows: the path inside the directory.
    label: String,
    added: usize,
    removed: usize,
    note: Option<RowNote>,
    comments: usize,
}

impl Entry {
    fn of(model: &Model, selection: &Selection<'_>, index: usize) -> Self {
        let view = model.view(index, selection.column);
        let file = view.file;
        let visited = model.is_visited(index);
        let inside = |path: &str| {
            path.strip_prefix(selection.directory)
                .unwrap_or(path)
                .to_string()
        };
        let label = match &file.rename_from {
            Some(old) => format!("{} \u{2192} {}", inside(old), inside(&file.new_path)),
            None => inside(display_path(file)),
        };
        let (added, removed) = if visited {
            (0, 0)
        } else {
            body_deltas(view.body)
        };
        Self {
            file: display_path(file).to_string(),
            status: (!visited).then(|| file_status(file)),
            label,
            added,
            removed,
            note: selection.notes.get(index).copied().flatten(),
            comments: selection.comments.count(display_path(file)),
        }
    }

    /// `M files/mod.rs  refactor ●  2 comments ······ +126 -63`
    fn header(&self, width: usize, theme: &Theme) -> DiffRow {
        let badge = match self.status {
            Some(status) => Span::styled(
                status.badge().to_string(),
                Style::default()
                    .fg(status_color(status, theme))
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::raw("\u{25cb}"),
        };
        let mut tail: Vec<Span<'static>> = Vec::new();
        if let Some(note) = &self.note {
            tail.push(Span::raw("  "));
            tail.extend(render_note(note, false, false, theme).spans);
        }
        if self.comments > 0 {
            tail.push(Span::styled(
                format!("  {}", plural(self.comments, "comment")),
                Style::default().fg(rgb_to_color(theme.muted)),
            ));
        }
        let counts = counts(self.added, self.removed, theme);
        let width_of = |spans: &[Span]| -> usize {
            spans.iter().map(|span| display_width(&span.content)).sum()
        };
        let (tail_width, counts_width) = (width_of(&tail), width_of(&counts));
        let room = width.saturating_sub(2 + tail_width + 2 + counts_width);
        let label = shorten_right(&self.label, room);
        let used = 2 + display_width(&label) + tail_width;
        let mut spans = vec![
            badge,
            Span::raw(" "),
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
        ];
        spans.extend(tail);
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + counts_width)),
        ));
        spans.extend(counts);
        let opens = Opens {
            path: self.file.clone(),
            at: None,
        };
        DiffRow::summary(Line::from(spans), LinePlace::summary(&self.file, 0), opens)
    }
}

/// `+a -r`, leaving out a zero side.
fn counts(added: usize, removed: usize, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if added > 0 {
        spans.push(Span::styled(
            format!("+{added}"),
            Style::default().fg(rgb_to_color(theme.status_added)),
        ));
    }
    if removed > 0 {
        if added > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("-{removed}"),
            Style::default().fg(rgb_to_color(theme.status_deleted)),
        ));
    }
    spans
}

/// `3 files changed  +12 -4`, leaving out files opened by a jump.
fn totals(entries: &[Entry], theme: &Theme) -> Line<'static> {
    let changed: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.status.is_some())
        .collect();
    let added: usize = changed.iter().map(|entry| entry.added).sum();
    let removed: usize = changed.iter().map(|entry| entry.removed).sum();
    Line::from(vec![
        Span::styled(
            format!("{} changed  ", plural(changed.len(), "file")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("+{added}"),
            Style::default().fg(rgb_to_color(theme.status_added)),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{removed}"),
            Style::default().fg(rgb_to_color(theme.status_deleted)),
        ),
    ])
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{MouseButton, MouseEventKind};

    use crate::cli::browse::comments::{CommentAnchor, LineSide};
    use crate::cli::browse::mode::Mode;
    use crate::cli::browse::test_screen::{self, LEFT, PANE_HEIGHT};

    use super::super::FilesMode;
    use super::super::model::ResolvedFile;
    use super::super::review;
    use super::super::review_tests::{fake_jev, wait_for_review};
    use super::super::test_support::*;

    fn rust(path: &str, before: &str, after: &str) -> ResolvedFile {
        ResolvedFile {
            file: file_diff(path),
            before: before.to_string(),
            after: after.to_string(),
        }
    }

    /// `src/a.rs` edits one line of `alpha`, `src/b.rs` adds a line to
    /// `Beta::run`, and `other.rs` sits outside `src/`.
    fn files() -> Vec<ResolvedFile> {
        vec![
            rust(
                "src/a.rs",
                "fn alpha() {\n    let x = 1;\n}\n",
                "fn alpha() {\n    let x = 2;\n}\n",
            ),
            rust(
                "src/b.rs",
                "struct Beta;\n\nimpl Beta {\n    fn run(&self) {\n        one();\n    }\n}\n",
                "struct Beta;\n\nimpl Beta {\n    fn run(&self) {\n        one();\n        two();\n    }\n}\n",
            ),
            resolved("other.rs"),
        ]
    }

    fn on_src(mut state: FilesMode) -> FilesMode {
        assert!(state.sidebar.select_directory_path("src/", 20));
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

    #[test]
    fn a_directory_shows_a_summary_of_its_files_instead_of_their_diffs() {
        let mut state = on_src(make_state(&files()));
        let rows = pane(&mut state);
        let text = rows.join("\n");

        assert!(rows[0].contains("─src/ ─"), "{}", rows[0]);
        assert!(text.contains("2 files changed  +2 -1"), "{text}");
        let (a, a_row) = row_with(&rows, "M a.rs");
        assert!(a_row.contains("+1 -1"), "{a_row}");
        assert!(rows[a + 1].contains("fn alpha"), "{}", rows[a + 1]);
        let (b, b_row) = row_with(&rows, "M b.rs");
        assert!(b_row.contains("+1"), "{b_row}");
        assert!(rows[b + 1].contains("impl Beta"), "{}", rows[b + 1]);
        assert!(rows[b + 2].contains("~ fn run(&self)"), "{}", rows[b + 2]);
        assert!(rows[b + 2].contains("body"), "{}", rows[b + 2]);
        assert!(rows[b + 2].contains("+1"), "{}", rows[b + 2]);
        assert!(!text.contains("other.rs"), "{text}");
        assert!(!text.contains("let x"), "no diff lines: {text}");
    }

    #[test]
    fn a_hunk_without_a_scope_names_its_lines_and_a_binary_says_so() {
        let mut state = on_src(make_state(&[
            resolved("src/notes.txt"),
            binary_resolved("src/logo.png"),
        ]));
        let rows = pane(&mut state);

        let (notes, _) = row_with(&rows, "notes.txt");
        assert!(rows[notes + 1].contains("line 1"), "{}", rows[notes + 1]);
        let (logo, _) = row_with(&rows, "logo.png");
        assert!(rows[logo + 1].contains("binary"), "{}", rows[logo + 1]);
    }

    /// A left press on pane row `y`.
    fn click(state: &mut FilesMode, y: usize) {
        Mode::handle_mouse(
            state,
            make_mouse(MouseEventKind::Down(MouseButton::Left), LEFT + 6, y as u16),
            PANE_HEIGHT,
        );
    }

    #[test]
    fn a_click_selects_an_entry_and_a_double_click_opens_its_file() {
        let mut state = on_src(make_state(&files()));
        let rows = pane(&mut state);
        let (y, _) = row_with(&rows, "M b.rs");

        click(&mut state, y);
        assert!(state.sidebar.selected_directory_path().is_some());
        let opens = state
            .unstaged
            .cursor_opens()
            .map(|opens| opens.path.clone());
        assert_eq!(opens.as_deref(), Some("src/b.rs"));

        click(&mut state, y);

        assert_eq!(
            selected_path(&state, &state.model).as_deref(),
            Some("src/b.rs")
        );
        assert!(state.sidebar.selected_directory_path().is_none());
        let rows = pane(&mut state);
        assert!(rows[0].contains("─src/b.rs "), "{}", rows[0]);
        assert!(rows.join("\n").contains("two();"));
    }

    #[test]
    fn double_clicking_a_hunk_opens_its_file_at_that_hunk() {
        let before: String = (1..=40).map(|n| format!("fn f{n}() {{}}\n")).collect();
        let after = before
            .replace("fn f3() {}", "fn f3() { three(); }")
            .replace("fn f30() {}", "fn f30() { thirty(); }");
        let mut state = on_src(make_state(&[rust("src/many.rs", &before, &after)]));
        let rows = pane(&mut state);
        let (y, _) = row_with(&rows, "fn f30");

        click(&mut state, y);
        click(&mut state, y);
        pane(&mut state);

        let anchor = state.unstaged.cursor_anchor().expect("cursor on a line");
        assert_eq!((anchor.path.as_str(), anchor.line), ("src/many.rs", 30));
    }

    #[test]
    fn entries_carry_the_review_note_and_the_comment_count() {
        let mut state = make_state(&files());
        state.review = review::Review::with_sender(fake_jev("src/a.rs"));
        state.start_review();
        wait_for_review(&mut state);
        state.comments.set(
            CommentAnchor {
                path: "src/b.rs".to_string(),
                side: LineSide::New,
                line: 6,
            },
            "why two?".to_string(),
            "        two();".to_string(),
            deltoids::LineKind::Added,
        );
        let mut state = on_src(state);
        let rows = pane(&mut state);

        let (_, a_row) = row_with(&rows, "M a.rs");
        assert!(a_row.contains("core ●"), "{a_row}");
        let (_, b_row) = row_with(&rows, "M b.rs");
        assert!(b_row.contains("refactor ●"), "{b_row}");
        assert!(b_row.contains("1 comment"), "{b_row}");
    }
}
