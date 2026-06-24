//! Shared hunk-header layout: decide breadcrumb box vs line-number box and
//! compute the geometry both renderers paint.
//!
//! `render.rs` (ANSI) and `render_tui.rs` (ratatui) are two painters over the
//! same `Hunk`. The header-kind decision (breadcrumb vs line-number box), the
//! row list, the `...` gap between non-adjacent ancestors, the line-number
//! column width, and the box content width are pure, in-process computation.
//! They live here once; each renderer matches [`HunkHeader`] exhaustively, so
//! adding a header kind is a compile error until every renderer handles it.

use unicode_width::UnicodeWidthStr;

use crate::Hunk;

const TAB_WIDTH: usize = 4;

/// The header drawn above a hunk's diff body. Renderers match exhaustively,
/// so adding a kind forces every renderer to handle it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HunkHeader {
    /// No enclosing structure: a small box with the new-file line number.
    LineNumber { line_num: usize },
    /// Breadcrumb of enclosing structural scopes.
    Breadcrumb(Breadcrumb),
}

/// Geometry for a breadcrumb box. Painters iterate `rows` and use the widths
/// to align the line-number column and the right box edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Breadcrumb {
    pub rows: Vec<BreadcrumbRow>,
    /// Width of the right-aligned line-number column (digits only).
    pub num_col_width: usize,
    /// Inner box width between the left edge and the ` │` / corner.
    pub content_width: usize,
}

/// One row of a breadcrumb box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BreadcrumbRow {
    /// One ancestor line: its start line number and source text.
    Scope { line_num: usize, text: String },
    /// A `...` gap between non-adjacent ancestors.
    Gap,
}

impl HunkHeader {
    /// Decide the header kind and compute its geometry. `max_width` is the
    /// render width; the breadcrumb box is capped to `max_width - 2`.
    pub fn plan(hunk: &Hunk, max_width: usize) -> HunkHeader {
        if hunk.ancestors.is_empty() {
            return HunkHeader::LineNumber {
                line_num: hunk.new_start,
            };
        }

        let max_content_width = max_width.saturating_sub(2); // room for " │"

        let max_line_num = hunk
            .ancestors
            .iter()
            .map(|a| a.start_line)
            .max()
            .unwrap_or(0);
        let num_col_width = max_line_num.to_string().len();

        let mut rows: Vec<BreadcrumbRow> = Vec::new();
        for (i, ancestor) in hunk.ancestors.iter().enumerate() {
            let gap_before = i > 0 && hunk.ancestors[i - 1].start_line + 1 < ancestor.start_line;
            if gap_before {
                rows.push(BreadcrumbRow::Gap);
            }
            rows.push(BreadcrumbRow::Scope {
                line_num: ancestor.start_line,
                text: ancestor.text.clone(),
            });
        }

        let prefix_width = num_col_width + 2; // "NNN: "
        let mut max_row_width = 0usize;
        for row in &rows {
            let row_width = match row {
                BreadcrumbRow::Scope { text, .. } => prefix_width + display_width(text),
                BreadcrumbRow::Gap => prefix_width + 3, // "..."
            };
            max_row_width = max_row_width.max(row_width);
        }
        let content_width = max_row_width.min(max_content_width);

        HunkHeader::Breadcrumb(Breadcrumb {
            rows,
            num_col_width,
            content_width,
        })
    }
}

impl Breadcrumb {
    /// Width of the `"NNN: "` prefix column.
    pub fn prefix_width(&self) -> usize {
        self.num_col_width + 2
    }
}

/// Canonical display width: tab counts as 4 columns, others by Unicode width.
pub(crate) fn display_width(s: &str) -> usize {
    let mut width = 0;
    for ch in s.chars() {
        if ch == '\t' {
            width += TAB_WIDTH;
        } else {
            width += UnicodeWidthStr::width(ch.to_string().as_str());
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffLine, LineKind, ScopeNode};

    fn hunk_with(ancestors: Vec<ScopeNode>, new_start: usize) -> Hunk {
        Hunk {
            old_start: new_start,
            new_start,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "x".to_string(),
            }],
            ancestors,
        }
    }

    fn scope(start_line: usize, text: &str) -> ScopeNode {
        ScopeNode {
            kind: "function_item".to_string(),
            name: "f".to_string(),
            start_line,
            end_line: start_line + 5,
            text: text.to_string(),
        }
    }

    #[test]
    fn plan_with_no_ancestors_is_line_number() {
        let hunk = hunk_with(Vec::new(), 42);
        assert_eq!(
            HunkHeader::plan(&hunk, 80),
            HunkHeader::LineNumber { line_num: 42 }
        );
    }

    #[test]
    fn plan_with_single_ancestor_has_one_scope_row() {
        let hunk = hunk_with(vec![scope(10, "fn main() {")], 10);
        let HunkHeader::Breadcrumb(b) = HunkHeader::plan(&hunk, 80) else {
            panic!("expected breadcrumb");
        };
        assert_eq!(
            b.rows,
            vec![BreadcrumbRow::Scope {
                line_num: 10,
                text: "fn main() {".to_string(),
            }]
        );
        assert_eq!(b.num_col_width, 2);
        // prefix (2 digits + ": ") + text width.
        assert_eq!(b.content_width, 4 + display_width("fn main() {"));
    }

    #[test]
    fn plan_inserts_gap_between_non_adjacent_ancestors() {
        let hunk = hunk_with(
            vec![scope(3, "impl Foo {"), scope(75, "    fn compute() {")],
            80,
        );
        let HunkHeader::Breadcrumb(b) = HunkHeader::plan(&hunk, 80) else {
            panic!("expected breadcrumb");
        };
        assert_eq!(
            b.rows,
            vec![
                BreadcrumbRow::Scope {
                    line_num: 3,
                    text: "impl Foo {".to_string(),
                },
                BreadcrumbRow::Gap,
                BreadcrumbRow::Scope {
                    line_num: 75,
                    text: "    fn compute() {".to_string(),
                },
            ]
        );
    }

    #[test]
    fn plan_no_gap_between_adjacent_ancestors() {
        let hunk = hunk_with(vec![scope(3, "impl Foo {"), scope(4, "    fn new() {")], 5);
        let HunkHeader::Breadcrumb(b) = HunkHeader::plan(&hunk, 80) else {
            panic!("expected breadcrumb");
        };
        assert!(!b.rows.iter().any(|r| matches!(r, BreadcrumbRow::Gap)));
    }

    #[test]
    fn plan_caps_content_width_to_max_width() {
        let long = "fn very_long_name_that_exceeds_the_small_render_width() {";
        let hunk = hunk_with(vec![scope(10, long)], 10);
        let HunkHeader::Breadcrumb(b) = HunkHeader::plan(&hunk, 20) else {
            panic!("expected breadcrumb");
        };
        assert_eq!(b.content_width, 20 - 2);
    }

    #[test]
    fn num_col_width_tracks_widest_line_number() {
        let hunk = hunk_with(vec![scope(7, "a {"), scope(1234, "    b {")], 1234);
        let HunkHeader::Breadcrumb(b) = HunkHeader::plan(&hunk, 200) else {
            panic!("expected breadcrumb");
        };
        assert_eq!(b.num_col_width, 4);
    }
}
