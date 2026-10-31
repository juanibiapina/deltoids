//! The change overview beside a diff's scrollbar: which cells of a track
//! show added or removed lines.
//!
//! Units are the caller's: the TUI passes rows with a cell of 1, the
//! reviewer passes CSS pixels with a cell of a few pixels.

/// The kind of change a span covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
}

/// A run of changed content, measured from the top of the content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub top: f64,
    pub height: f64,
    pub change: Change,
}

/// One cell of the track. A cell can hold both kinds of change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    pub added: bool,
    pub removed: bool,
}

/// Map `spans` onto `track / cell` cells.
///
/// Content that fits the track maps one-to-one, so marks line up with the
/// rows beside them. Taller content scales so that its first and last
/// positions land on the first and last cells, keeping changes at either end
/// visible at every scroll position.
pub fn change_cells(spans: &[Span], content: f64, track: f64, cell: f64) -> Vec<Cell> {
    let count = if cell > 0.0 && track > 0.0 {
        (track / cell).floor() as usize
    } else {
        0
    };
    let mut cells = vec![Cell::default(); count];
    if count == 0 || content <= 0.0 {
        return cells;
    }
    let last_cell = count - 1;
    let fits = content <= track;
    let cell_at = |y: f64| -> usize {
        let clamped = y.clamp(0.0, content - 1.0).max(0.0);
        let index = if fits {
            (clamped / cell).floor()
        } else if content <= 1.0 {
            0.0
        } else {
            (clamped * last_cell as f64 / (content - 1.0)).floor()
        };
        (index as usize).min(last_cell)
    };
    for span in spans.iter().filter(|span| span.height > 0.0) {
        let first = cell_at(span.top);
        let last = cell_at(span.top + span.height - 1.0);
        for cell in &mut cells[first..=last.max(first)] {
            match span.change {
                Change::Added => cell.added = true,
                Change::Removed => cell.removed = true,
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(top: f64, height: f64, change: Change) -> Span {
        Span {
            top,
            height,
            change,
        }
    }

    fn marks(cells: &[Cell]) -> String {
        cells
            .iter()
            .map(|c| match (c.added, c.removed) {
                (true, true) => 'b',
                (true, false) => '+',
                (false, true) => '-',
                (false, false) => '.',
            })
            .collect()
    }

    #[test]
    fn tall_content_keeps_changes_at_both_ends_and_the_middle() {
        let spans = [
            span(0.0, 20.0, Change::Removed),
            span(5000.0, 20.0, Change::Added),
            span(9980.0, 20.0, Change::Removed),
        ];
        assert_eq!(
            marks(&change_cells(&spans, 10000.0, 30.0, 3.0)),
            "-...+...--"
        );
    }

    #[test]
    fn an_added_and_a_removed_span_sharing_a_cell_keep_both() {
        let spans = [
            span(0.0, 10.0, Change::Added),
            span(10.0, 10.0, Change::Removed),
        ];
        assert_eq!(
            change_cells(&spans, 10000.0, 30.0, 3.0)[0],
            Cell {
                added: true,
                removed: true
            }
        );
    }

    #[test]
    fn fitting_content_lines_up_with_the_rows() {
        let spans = [
            span(0.0, 3.0, Change::Added),
            span(9.0, 6.0, Change::Removed),
        ];
        assert_eq!(marks(&change_cells(&spans, 20.0, 30.0, 3.0)), "+..--.....");
    }

    #[test]
    fn rows_of_one_unit_scale_by_their_endpoints() {
        let spans = [
            span(0.0, 1.0, Change::Removed),
            span(50.0, 1.0, Change::Added),
            span(100.0, 1.0, Change::Removed),
        ];
        assert_eq!(marks(&change_cells(&spans, 101.0, 5.0, 1.0)), "-.+.-");
    }

    #[test]
    fn degenerate_geometry_gives_empty_cells() {
        let spans = [span(0.0, 1.0, Change::Added)];
        assert_eq!(
            change_cells(&spans, 0.0, 30.0, 3.0),
            vec![Cell::default(); 10]
        );
        assert!(change_cells(&spans, 100.0, 0.0, 3.0).is_empty());
        assert!(change_cells(&spans, 1.0, 2.0, 3.0).is_empty());
        assert!(change_cells(&spans, 10.0, 10.0, 0.0).is_empty());
    }
}
