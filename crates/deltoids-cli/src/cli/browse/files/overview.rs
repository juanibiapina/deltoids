//! The directory overview: what the diff pane shows while the sidebar
//! selects a directory. Each file of the shown column gets a header (its
//! status, path inside the directory, review note, comment count, and
//! line counts), then one line per hunk naming the scope the hunk sits
//! in, with its own counts. A click on a header opens the file; a click on
//! a hunk opens the file at that hunk.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::render_tui::rgb_to_color;
use deltoids::{Hunk, LineKind, ScopeNode, Theme};

use crate::cli::browse::comments::{CommentAnchor, CommentStore, Numbering, numbered_lines};
use crate::cli::browse::diff_cursor::DiffRow;
use crate::cli::browse::text::display_width;
use crate::sidebar::{FileStatus, RowNote, display_path, file_status, render_note, status_color};

use super::model::{Column, FileBody, Model, body_deltas};

const HUNK_INDENT: &str = "  ";

/// The overview's rows and what a click on each opens.
pub(super) struct Overview {
    pub(super) rows: Vec<DiffRow>,
    /// Indexed like `rows`; `None` for the totals and spacer rows.
    pub(super) opens: Vec<Option<Open>>,
}

/// Where a click on an overview row goes: file `file` (a model index),
/// with the cursor on `at`, a hunk's first changed line, when set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Open {
    pub(super) file: usize,
    pub(super) at: Option<CommentAnchor>,
}

/// What the overview needs besides the model: the selected directory
/// (`src/cli/`, as the sidebar names it), the shown column's files in
/// sidebar order, each file's review note by model index, and the
/// session's comments.
pub(super) struct Selection<'a> {
    pub(super) directory: &'a str,
    pub(super) column: Column,
    pub(super) files: &'a [usize],
    pub(super) notes: &'a [Option<RowNote>],
    pub(super) comments: &'a CommentStore,
}

/// Lay out the overview of `selection` for a pane `width` columns wide.
pub(super) fn build(
    model: &Model,
    selection: &Selection<'_>,
    width: usize,
    theme: &Theme,
) -> Overview {
    let entries: Vec<Entry> = selection
        .files
        .iter()
        .map(|&index| Entry::of(model, selection, index))
        .collect();
    let counts = Counts::fit(&entries);
    let mut overview = Overview {
        rows: Vec::new(),
        opens: Vec::new(),
    };
    overview.push(totals(&entries, theme), None);
    for entry in &entries {
        overview.push(Line::default(), None);
        let open = Open {
            file: entry.index,
            at: None,
        };
        overview.push(entry.header(&counts, width, theme), Some(open));
        match &entry.body {
            Body::Hunks(hunks) => {
                for hunk in hunks {
                    let open = Open {
                        file: entry.index,
                        at: hunk.at.clone(),
                    };
                    overview.push(hunk.line(&counts, width, theme), Some(open));
                }
            }
            Body::Note(note) => {
                let open = Open {
                    file: entry.index,
                    at: None,
                };
                overview.push(muted_line(note, width, theme), Some(open));
            }
        }
    }
    overview
}

impl Overview {
    fn push(&mut self, line: Line<'static>, opens: Option<Open>) {
        self.rows.push(DiffRow::plain(line));
        self.opens.push(opens);
    }
}

/// One file's facts, gathered before layout.
struct Entry {
    index: usize,
    /// `None` for a file opened by a jump.
    status: Option<FileStatus>,
    path: String,
    added: usize,
    removed: usize,
    note: Option<RowNote>,
    comments: usize,
    body: Body,
}

/// What goes under a file's header.
enum Body {
    Hunks(Vec<HunkEntry>),
    /// One muted line for a file without hunks to list.
    Note(String),
}

struct HunkEntry {
    scope: String,
    added: usize,
    removed: usize,
    at: Option<CommentAnchor>,
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
        let path = match &file.rename_from {
            Some(old) => format!("{} \u{2192} {}", inside(old), inside(&file.new_path)),
            None => inside(display_path(file)),
        };
        let (added, removed) = if visited {
            (0, 0)
        } else {
            body_deltas(view.body)
        };
        let body = if visited {
            Body::Note("visited".to_string())
        } else {
            body_of(view.body, display_path(file), view.numbering)
        };
        Self {
            index,
            status: (!visited).then(|| file_status(file)),
            path,
            added,
            removed,
            note: selection.notes.get(index).copied().flatten(),
            comments: selection.comments.count(display_path(file)),
            body,
        }
    }

    /// `M files/mod.rs  refactor ●  2 comments ······ +126 -63`
    fn header(&self, counts: &Counts, width: usize, theme: &Theme) -> Line<'static> {
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
        let tail_width: usize = tail.iter().map(|span| display_width(&span.content)).sum();
        let room = width.saturating_sub(2 + tail_width + 2 + counts.width());
        let path = shorten_right(&self.path, room);
        let used = 2 + display_width(&path) + tail_width;
        let mut spans = vec![
            badge,
            Span::raw(" "),
            Span::styled(path, Style::default().add_modifier(Modifier::BOLD)),
        ];
        spans.extend(tail);
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + counts.width())),
        ));
        spans.extend(counts.spans(self.added, self.removed, theme));
        Line::from(spans)
    }
}

impl HunkEntry {
    /// `  impl DiffPane › fn new ······ +1`
    fn line(&self, counts: &Counts, width: usize, theme: &Theme) -> Line<'static> {
        let room = width.saturating_sub(HUNK_INDENT.len() + 2 + counts.width());
        let scope = shorten_right(&self.scope, room);
        let used = HUNK_INDENT.len() + display_width(&scope);
        let mut spans = vec![Span::raw(HUNK_INDENT), Span::raw(scope)];
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + counts.width())),
        ));
        spans.extend(counts.spans(self.added, self.removed, theme));
        Line::from(spans)
    }
}

/// The hunks of a file's body, or a note for a body without any.
fn body_of(body: &FileBody, path: &str, numbering: Numbering) -> Body {
    match body {
        FileBody::Diff(diff) if diff.hunks().is_empty() => {
            Body::Note("no text changes".to_string())
        }
        FileBody::Diff(diff) => Body::Hunks(
            diff.hunks()
                .iter()
                .map(|hunk| HunkEntry::of(hunk, path, numbering))
                .collect(),
        ),
        FileBody::Symlink(_) => Body::Note("symlink".to_string()),
        FileBody::Binary => Body::Note("binary".to_string()),
        FileBody::StatusOnly => Body::Note("staged and unstaged changes cancel out".to_string()),
        FileBody::Submodule { .. } => Body::Note("submodule".to_string()),
    }
}

impl HunkEntry {
    fn of(hunk: &Hunk, path: &str, numbering: Numbering) -> Self {
        let kinds = || hunk.lines.iter().map(|line| &line.kind);
        let at = numbered_lines(hunk, numbering)
            .find(|line| *line.kind != LineKind::Context)
            .map(|line| CommentAnchor {
                path: path.to_string(),
                side: line.side,
                line: line.number,
            });
        Self {
            scope: scope_label(hunk),
            added: kinds().filter(|kind| **kind == LineKind::Added).count(),
            removed: kinds().filter(|kind| **kind == LineKind::Removed).count(),
            at,
        }
    }
}

/// The hunk's two innermost scopes as their source reads them
/// (`impl DiffPane › fn new`), or the lines it covers when it sits in
/// no scope.
fn scope_label(hunk: &Hunk) -> String {
    let start = hunk.ancestors.len().saturating_sub(2);
    let scopes: Vec<String> = hunk.ancestors[start..]
        .iter()
        .map(scope_text)
        .filter(|text| !text.is_empty())
        .collect();
    if !scopes.is_empty() {
        return scopes.join(" \u{203a} ");
    }
    let lines = hunk
        .lines
        .iter()
        .filter(|line| line.kind != LineKind::Removed)
        .count();
    match lines {
        0 | 1 => format!("line {}", hunk.new_start),
        _ => format!(
            "lines {}\u{2013}{}",
            hunk.new_start,
            hunk.new_start + lines - 1
        ),
    }
}

/// A scope's first source line, short: up to and including its name, so
/// `fn render(&self) -> Line {` reads `fn render`, without a leading
/// `pub` or `export`. A scope without a name keeps its line, minus an
/// opening `{`.
fn scope_text(scope: &ScopeNode) -> String {
    let text = scope.text.trim();
    let text = match (!scope.name.is_empty())
        .then(|| text.find(scope.name.as_str()))
        .flatten()
    {
        Some(at) => &text[..at + scope.name.len()],
        None => text.trim_end_matches('{').trim_end(),
    };
    strip_visibility(text).to_string()
}

fn strip_visibility(text: &str) -> &str {
    let mut text = text;
    loop {
        let rest = if let Some(rest) = text.strip_prefix("pub(") {
            rest.find(") ").map(|end| &rest[end + 2..])
        } else {
            ["pub ", "export default ", "export "]
                .iter()
                .find_map(|prefix| text.strip_prefix(prefix))
        };
        match rest {
            Some(rest) => text = rest,
            None => return text,
        }
    }
}

/// The `+added -removed` column, right-aligned at the pane's edge and
/// shared by every header and hunk line so the numbers line up.
struct Counts {
    added: usize,
    removed: usize,
}

impl Counts {
    fn fit(entries: &[Entry]) -> Self {
        let digits = |lines: usize| {
            if lines == 0 {
                0
            } else {
                lines.to_string().len() + 1
            }
        };
        Self {
            added: entries
                .iter()
                .map(|entry| digits(entry.added))
                .max()
                .unwrap_or(0),
            removed: entries
                .iter()
                .map(|entry| digits(entry.removed))
                .max()
                .unwrap_or(0),
        }
    }

    fn width(&self) -> usize {
        self.added + usize::from(self.added > 0 && self.removed > 0) + self.removed
    }

    fn spans(&self, added: usize, removed: usize, theme: &Theme) -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled(
            format!("{:>width$}", count("+", added), width = self.added),
            Style::default().fg(rgb_to_color(theme.status_added)),
        )];
        if self.added > 0 && self.removed > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("{:>width$}", count("-", removed), width = self.removed),
            Style::default().fg(rgb_to_color(theme.status_deleted)),
        ));
        spans
    }
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

fn muted_line(text: &str, width: usize, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw(HUNK_INDENT),
        Span::styled(
            shorten_right(text, width.saturating_sub(HUNK_INDENT.len())),
            Style::default().fg(rgb_to_color(theme.muted)),
        ),
    ])
}

fn count(sign: &str, lines: usize) -> String {
    if lines == 0 {
        String::new()
    } else {
        format!("{sign}{lines}")
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `text` cut to `width` columns from the right, with `…` at the end
/// when it was cut.
fn shorten_right(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut used = 1;
    for ch in text.chars() {
        let w = display_width(&ch.to_string());
        if used + w > width {
            break;
        }
        used += w;
        kept.push(ch);
    }
    kept.push('\u{2026}');
    kept
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
        assert!(
            rows[b + 1].contains("impl Beta › fn run"),
            "{}",
            rows[b + 1]
        );
        assert!(rows[b + 1].contains("+1"), "{}", rows[b + 1]);
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

    #[test]
    fn clicking_an_entry_opens_its_file() {
        let mut state = on_src(make_state(&files()));
        let rows = pane(&mut state);
        let (y, _) = row_with(&rows, "M b.rs");

        Mode::handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Down(MouseButton::Left), LEFT + 6, y as u16),
            PANE_HEIGHT,
        );

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
    fn clicking_a_hunk_opens_its_file_at_that_hunk() {
        let before: String = (1..=40).map(|n| format!("fn f{n}() {{}}\n")).collect();
        let after = before
            .replace("fn f3() {}", "fn f3() { three(); }")
            .replace("fn f30() {}", "fn f30() { thirty(); }");
        let mut state = on_src(make_state(&[rust("src/many.rs", &before, &after)]));
        let rows = pane(&mut state);
        let (y, _) = row_with(&rows, "fn f30");

        Mode::handle_mouse(
            &mut state,
            make_mouse(MouseEventKind::Down(MouseButton::Left), LEFT + 6, y as u16),
            PANE_HEIGHT,
        );
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
