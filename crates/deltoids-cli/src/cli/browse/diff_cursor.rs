//! The diff pane's rows and the cursor that walks them.
//!
//! A rendered diff row is not the same thing as a diff line: a long line
//! wraps onto several rows, and headers, spacers, and inline comments own
//! rows of their own. A [`DiffRow`] that *starts* a diff line carries two
//! separate identities for it:
//!
//! - [`LinePlace`] — where the line sits in what was drawn. Unique by
//!   construction, since it counts rendered structure (file, hunk, line
//!   within the hunk) rather than file lines. This is what the cursor
//!   uses, so navigation cannot be confused by anything the diff engine
//!   does with hunk numbering: overlapping, repeated, or misnumbered
//!   hunks all still walk cleanly.
//! - [`CommentAnchor`] — which line of which file the reviewer is talking
//!   about. Semantic, and deliberately *not* unique: the same file line
//!   rendered in two hunks is one line and carries one comment.
//!
//! Both modes drive the same cursor through [`step_cursor`], so `j`/`k`
//! feel identical in the working-tree diff and the trace browser.

use ratatui::text::Line;

use super::comments::CommentAnchor;

/// Where a rendered diff line sits in the drawn structure.
///
/// Unique per rendered line: `file` names the file (or trace entry),
/// `hunk` its position in that file's hunk list, and `index` the line's
/// position inside the hunk. None of these come from the diff's line
/// numbering or the model's shifting file indices, so the cursor keeps
/// working however the hunks are shaped or files enter and leave the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LinePlace {
    pub(super) file: String,
    pub(super) hunk: usize,
    pub(super) index: usize,
}

/// One rendered row of a diff pane plus what it points at.
#[derive(Debug, Clone)]
pub(super) struct DiffRow {
    pub(super) line: Line<'static>,
    /// The file line this row renders. Every row of a wrapped line
    /// carries it; `None` for header, spacer, and inline-comment rows.
    pub(super) anchor: Option<CommentAnchor>,
    /// Set on the row that *starts* the line: the cursor's stop.
    pub(super) place: Option<LinePlace>,
    /// Set on the row that *ends* the line: where an inline comment goes.
    pub(super) ends_line: bool,
}

impl DiffRow {
    /// A row the cursor skips.
    pub(super) fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            anchor: None,
            place: None,
            ends_line: false,
        }
    }

    /// One row of a diff line. `place` is set only on the line's first
    /// row (the cursor stops once per line) and `ends_line` only on its
    /// last (an inline comment goes after the whole wrapped line).
    pub(super) fn line_row(
        line: Line<'static>,
        anchor: CommentAnchor,
        place: Option<LinePlace>,
        ends_line: bool,
    ) -> Self {
        Self {
            line,
            anchor: Some(anchor),
            place,
            ends_line,
        }
    }

    /// Whether the diff cursor stops on this row.
    pub(super) fn is_selectable(&self) -> bool {
        self.place.is_some()
    }
}

/// Which way [`step_cursor`] moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Down,
    Up,
}

/// Where the cursor is, how the pane is scrolled, and which rendered line
/// it sits on. Panes keep one of these; the helpers here are the only
/// things that move it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Cursor {
    pub(super) row: usize,
    pub(super) scroll: usize,
    /// The line the cursor is on, so a rebuilt window can put it back
    /// there. `None` before the first draw.
    place: Option<LinePlace>,
}

impl Cursor {
    /// Record the line the cursor now sits on. Every move ends here, so a
    /// pane can never forget to keep the two in step.
    fn sync_place(&mut self, rows: &[DiffRow]) {
        self.place = rows.get(self.row).and_then(|row| row.place.clone());
    }
}

/// Put the cursor back after the window was rebuilt: on the same rendered
/// line when it is still drawn, otherwise on the nearest line to where it
/// was.
///
/// Panes re-assemble their window on every draw, so this runs constantly.
/// Resolving by [`LinePlace`] is what keeps the cursor still while the
/// rows around it move — during a reload, a width change, or a comment
/// being added above it.
pub(super) fn restore_cursor(rows: &[DiffRow], cursor: &mut Cursor) {
    cursor.row = match cursor
        .place
        .as_ref()
        .and_then(|place| row_for_place(rows, place))
    {
        Some(row) => row,
        None => snap_to_selectable(rows, cursor.row),
    };
    cursor.sync_place(rows);
}

/// The row drawing `place`, if it is still on screen.
fn row_for_place(rows: &[DiffRow], place: &LinePlace) -> Option<usize> {
    rows.iter()
        .position(|row| row.place.as_ref() == Some(place))
}

/// Put the cursor on `row` when that row starts a diff line (a click).
pub(super) fn select_row(rows: &[DiffRow], row: usize, cursor: &mut Cursor) {
    if rows.get(row).is_some_and(DiffRow::is_selectable) {
        cursor.row = row;
        cursor.sync_place(rows);
    }
}

/// Move the cursor one diff line in `step` and scroll just enough to keep
/// it in view.
///
/// When the cursor is currently outside the viewport (the user scrolled
/// away with `J`/`K`, `PgDn`, or `G`), the first press instead brings it
/// to the nearest diff line *inside* the viewport, so navigating never
/// yanks the view back to where the cursor was left.
pub(super) fn step_cursor(rows: &[DiffRow], step: Step, height: usize, cursor: &mut Cursor) {
    let height = height.max(1);
    if let Some(row) = row_inside_viewport(rows, cursor, height, step) {
        cursor.row = row;
        cursor.sync_place(rows);
        return;
    }
    let next = match step {
        Step::Down => (cursor.row + 1..rows.len()).find(|&i| rows[i].is_selectable()),
        Step::Up => (0..cursor.row.min(rows.len()))
            .rev()
            .find(|&i| rows[i].is_selectable()),
    };
    if let Some(next) = next {
        cursor.row = next;
        keep_visible(cursor, height);
    }
    cursor.sync_place(rows);
}

/// When the cursor sits outside the visible window, the selectable row to
/// jump to: the first one from the top when moving down, the last one
/// from the bottom when moving up. `None` when the cursor is already
/// visible (or nothing in view is selectable).
fn row_inside_viewport(
    rows: &[DiffRow],
    cursor: &Cursor,
    height: usize,
    step: Step,
) -> Option<usize> {
    let end = cursor.scroll.saturating_add(height).min(rows.len());
    if cursor.row >= cursor.scroll && cursor.row < end {
        return None;
    }
    let visible = cursor.scroll..end;
    match step {
        Step::Down => visible.clone().find(|&i| rows[i].is_selectable()),
        Step::Up => visible.rev().find(|&i| rows[i].is_selectable()),
    }
}

/// Scroll the pane just enough to bring the cursor row into view.
pub(super) fn keep_visible(cursor: &mut Cursor, height: usize) {
    if cursor.row < cursor.scroll {
        cursor.scroll = cursor.row;
    } else if cursor.row >= cursor.scroll + height {
        cursor.scroll = cursor.row + 1 - height;
    }
}

/// The row the cursor should sit on after `rows` was rebuilt: itself when
/// it still starts a diff line, else the nearest such row after it, else
/// the nearest before it.
pub(super) fn snap_to_selectable(rows: &[DiffRow], row: usize) -> usize {
    if rows.get(row).is_some_and(DiffRow::is_selectable) {
        return row;
    }
    if let Some(index) = (row..rows.len()).find(|&i| rows[i].is_selectable()) {
        return index;
    }
    (0..row.min(rows.len()))
        .rev()
        .find(|&i| rows[i].is_selectable())
        .unwrap_or(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::comments::{CommentScope, LineSide};

    fn anchor(line: usize) -> CommentAnchor {
        anchor_in("a.rs", line)
    }

    fn anchor_in(path: &str, line: usize) -> CommentAnchor {
        CommentAnchor {
            scope: CommentScope::TraceEntry {
                trace_id: "T1".to_string(),
                entry_index: 0,
            },
            path: path.to_string(),
            side: LineSide::New,
            line,
        }
    }

    fn place(index: usize) -> LinePlace {
        LinePlace {
            file: "a.rs".to_string(),
            hunk: 0,
            index,
        }
    }

    fn diff_line(text: &str, index: usize) -> DiffRow {
        DiffRow::line_row(
            Line::from(text.to_string()),
            anchor(index + 1),
            Some(place(index)),
            true,
        )
    }

    /// Rows shaped like a real pane: a header, then diff lines, one of
    /// which wraps onto a continuation row.
    fn rows() -> Vec<DiffRow> {
        vec![
            DiffRow::plain(Line::from("header")),
            diff_line("line 1", 0),
            diff_line("line 2", 1),
            DiffRow::plain(Line::from("…wrapped")),
            DiffRow::plain(Line::from("")),
            diff_line("line 3", 2),
        ]
    }

    fn cursor_at(rows: &[DiffRow], row: usize, scroll: usize) -> Cursor {
        let mut cursor = Cursor {
            row,
            scroll,
            place: None,
        };
        cursor.sync_place(rows);
        cursor
    }

    #[test]
    fn stepping_visits_only_rows_that_start_a_diff_line() {
        let rows = rows();
        let mut cursor = cursor_at(&rows, 1, 0);

        step_cursor(&rows, Step::Down, 10, &mut cursor);
        assert_eq!(cursor.row, 2);
        // Skips the wrapped continuation and the spacer.
        step_cursor(&rows, Step::Down, 10, &mut cursor);
        assert_eq!(cursor.row, 5);
        // Nothing selectable below: the cursor stays put.
        step_cursor(&rows, Step::Down, 10, &mut cursor);
        assert_eq!(cursor.row, 5);

        step_cursor(&rows, Step::Up, 10, &mut cursor);
        assert_eq!(cursor.row, 2);
    }

    #[test]
    fn stepping_scrolls_to_keep_the_cursor_visible() {
        let rows = rows();
        let mut cursor = cursor_at(&rows, 1, 0);
        // A two-row viewport forces the pane to follow the cursor.
        step_cursor(&rows, Step::Down, 2, &mut cursor);
        step_cursor(&rows, Step::Down, 2, &mut cursor);
        assert_eq!(cursor.row, 5);
        assert!(cursor.row >= cursor.scroll && cursor.row < cursor.scroll + 2);

        step_cursor(&rows, Step::Up, 2, &mut cursor);
        assert!(cursor.row >= cursor.scroll && cursor.row < cursor.scroll + 2);
    }

    #[test]
    fn a_cursor_scrolled_out_of_view_returns_to_the_visible_window() {
        let rows = rows();
        // The user scrolled down past the cursor with J / PgDn.
        let mut cursor = cursor_at(&rows, 1, 4);
        step_cursor(&rows, Step::Down, 2, &mut cursor);
        assert_eq!(cursor.row, 5, "jumps into view instead of scrolling back");
        assert_eq!(cursor.scroll, 4, "the view the user chose is kept");

        // Same in the other direction: scrolled up past the cursor.
        let mut cursor = cursor_at(&rows, 5, 0);
        step_cursor(&rows, Step::Up, 3, &mut cursor);
        assert_eq!(cursor.row, 2);
        assert_eq!(cursor.scroll, 0);
    }

    #[test]
    fn snapping_prefers_the_next_diff_line_then_the_previous() {
        let rows = rows();
        // A header row snaps forward to the first diff line.
        assert_eq!(snap_to_selectable(&rows, 0), 1);
        // A spacer snaps forward too.
        assert_eq!(snap_to_selectable(&rows, 4), 5);
        // Past the end there is nothing forward, so it snaps back.
        assert_eq!(snap_to_selectable(&rows, 99), 5);
        // A row that already starts a diff line stays put.
        assert_eq!(snap_to_selectable(&rows, 2), 2);
        // With nothing selectable at all the row is left alone.
        assert_eq!(snap_to_selectable(&[DiffRow::plain(Line::from("x"))], 0), 0);
    }

    #[test]
    fn a_rebuilt_window_keeps_the_cursor_on_its_line() {
        let rows = rows();
        let mut cursor = cursor_at(&rows, 5, 0);

        // The same lines, but two extra rows above them (a comment was
        // added higher up, or the pane got narrower and a line wrapped).
        let mut rebuilt = vec![
            DiffRow::plain(Line::from("header")),
            DiffRow::plain(Line::from("▌ a comment")),
        ];
        rebuilt.extend(rows.iter().skip(1).cloned());

        restore_cursor(&rebuilt, &mut cursor);
        assert_eq!(cursor.row, 6, "followed its line down two rows");
        assert_eq!(rebuilt[cursor.row].place, Some(place(2)));
    }

    #[test]
    fn a_new_file_before_the_current_file_does_not_move_the_cursor() {
        let original = vec![DiffRow::line_row(
            Line::from("b line"),
            anchor_in("b.rs", 1),
            Some(LinePlace {
                file: "b.rs".to_string(),
                hunk: 0,
                index: 0,
            }),
            true,
        )];
        let mut cursor = cursor_at(&original, 0, 0);
        let rebuilt = vec![
            DiffRow::line_row(
                Line::from("a line"),
                anchor_in("a.rs", 1),
                Some(LinePlace {
                    file: "a.rs".to_string(),
                    hunk: 0,
                    index: 0,
                }),
                true,
            ),
            DiffRow::line_row(
                Line::from("b line"),
                anchor_in("b.rs", 1),
                Some(LinePlace {
                    file: "b.rs".to_string(),
                    hunk: 0,
                    index: 0,
                }),
                true,
            ),
        ];

        restore_cursor(&rebuilt, &mut cursor);

        assert_eq!(rebuilt[cursor.row].anchor.as_ref().unwrap().path, "b.rs");
    }

    #[test]
    fn a_line_that_is_gone_falls_back_to_the_nearest_one() {
        let rows = rows();
        let mut cursor = cursor_at(&rows, 5, 0);

        // The cursor's line is no longer drawn (its file left the diff).
        let rebuilt = vec![DiffRow::plain(Line::from("header")), diff_line("other", 7)];
        restore_cursor(&rebuilt, &mut cursor);
        assert_eq!(cursor.row, 1);
        assert_eq!(rebuilt[cursor.row].place, Some(place(7)));
    }

    #[test]
    fn the_cursor_is_immune_to_repeated_line_numbers() {
        // Overlapping hunks can render one file line twice: same anchor,
        // different places. Navigation must not care.
        let other_hunk = |index: usize| LinePlace {
            file: "a.rs".to_string(),
            hunk: 1,
            index,
        };
        let rows = vec![
            diff_line("line 1", 0),
            DiffRow::line_row(
                Line::from("line 1 again"),
                anchor(1),
                Some(other_hunk(0)),
                true,
            ),
            DiffRow::line_row(Line::from("line 2"), anchor(2), Some(other_hunk(1)), true),
        ];
        let mut cursor = cursor_at(&rows, 0, 0);

        for expected in [1, 2] {
            step_cursor(&rows, Step::Down, 10, &mut cursor);
            assert_eq!(cursor.row, expected);
            // Every draw re-resolves the cursor against fresh rows.
            restore_cursor(&rows, &mut cursor);
            assert_eq!(
                cursor.row, expected,
                "the redraw left the cursor where the user put it"
            );
        }
    }

    #[test]
    fn clicking_selects_only_rows_that_start_a_diff_line() {
        let rows = rows();
        let mut cursor = cursor_at(&rows, 1, 0);

        select_row(&rows, 5, &mut cursor);
        assert_eq!(cursor.row, 5);
        assert_eq!(cursor.place, Some(place(2)));

        // A header, spacer, or wrapped continuation is not a stop.
        select_row(&rows, 3, &mut cursor);
        assert_eq!(cursor.row, 5, "the click was ignored");
    }
}
