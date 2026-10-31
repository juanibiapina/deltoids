//! Diff sessions: one file's diff kept alive in the module so the reviewer
//! can grow and shrink hunk context (the TUI's `z`/`x`) and re-render it in
//! any theme without recomputing the diff.
//!
//! The module is single-threaded, so the table is a thread local. Ids start
//! at 1; 0 never names a session.

use std::cell::RefCell;
use std::collections::HashMap;

use deltoids::Diff;
use deltoids::render_html::render_file_html;

use crate::{before_from_patch, theme_opt};

struct Session {
    diff: Diff,
    new_lines: usize,
}

#[derive(Default)]
struct Table {
    next: u32,
    open: HashMap<u32, Session>,
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

/// How a reshape went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reshape {
    Changed,
    Unchanged,
    Unknown,
}

/// Open a session on the diff of `before` → `after`.
pub fn open(before: &str, after: &str, path: &str) -> u32 {
    let session = Session {
        diff: Diff::compute(before, after, path),
        new_lines: after.lines().count(),
    };
    TABLE.with_borrow_mut(|table| {
        table.next += 1;
        table.open.insert(table.next, session);
        table.next
    })
}

/// Open a session from `after` and a unified `patch`, as
/// [`crate::render_html_from_patch`] renders.
pub fn open_from_patch(after: &str, patch: &str, path: &str) -> u32 {
    open(&before_from_patch(after, patch), after, path)
}

/// The session's current diff as HTML, or `None` for an unknown id.
pub fn html(id: u32, theme: &str) -> Option<String> {
    TABLE.with_borrow(|table| {
        let session = table.open.get(&id)?;
        Some(render_file_html(
            session.diff.hunks(),
            session.diff.highlight(),
            theme_opt(theme),
            session.new_lines,
        ))
    })
}

fn reshape(id: u32, step: impl FnOnce(&Diff) -> Option<Diff>) -> Reshape {
    TABLE.with_borrow_mut(|table| match table.open.get_mut(&id) {
        None => Reshape::Unknown,
        Some(session) => match step(&session.diff) {
            Some(diff) => {
                session.diff = diff;
                Reshape::Changed
            }
            None => Reshape::Unchanged,
        },
    })
}

/// Grow hunk `hunk`'s context one scope level ([`Diff::expand`]).
pub fn expand(id: u32, hunk: usize) -> Reshape {
    reshape(id, |diff| diff.expand(hunk))
}

/// Undo the latest growth that reached hunk `hunk` ([`Diff::shrink`]).
pub fn shrink(id: u32, hunk: usize) -> Reshape {
    reshape(id, |diff| diff.shrink(hunk))
}

/// Forget a session. Unknown ids are ignored.
pub fn close(id: u32) {
    TABLE.with_borrow_mut(|table| {
        table.open.remove(&id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "fn a() {\n    1;\n}\n\nfn b() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    let w = 4;\n    let v = 5;\n    x + y\n}\n";
    const AFTER: &str = "fn a() {\n    1;\n}\n\nfn b() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    let w = 4;\n    let v = 5;\n    x + y + 1\n}\n";

    fn context_rows(id: u32) -> usize {
        html(id, "")
            .unwrap()
            .matches("class=\"row context\"")
            .count()
    }

    #[test]
    fn expand_grows_the_hunk_and_shrink_restores_it() {
        let id = open(BEFORE, AFTER, "src/lib.rs");
        let first = html(id, "").unwrap();
        assert!(first.contains("data-hunk=\"0\""));
        let rows = context_rows(id);

        assert_eq!(expand(id, 0), Reshape::Changed);
        assert!(context_rows(id) > rows);

        assert_eq!(shrink(id, 0), Reshape::Changed);
        assert_eq!(html(id, "").unwrap(), first);
        assert_eq!(shrink(id, 0), Reshape::Unchanged);
        close(id);
    }

    #[test]
    fn a_session_renders_like_a_one_shot_render_in_every_theme() {
        let id = open(BEFORE, AFTER, "src/lib.rs");
        for theme in ["", "GitHub"] {
            assert_eq!(
                html(id, theme).unwrap(),
                crate::render_html(BEFORE, AFTER, "src/lib.rs", theme)
            );
        }
        close(id);
    }

    #[test]
    fn unknown_and_closed_ids_report_unknown() {
        let id = open(BEFORE, AFTER, "src/lib.rs");
        close(id);
        assert_eq!(html(id, ""), None);
        assert_eq!(expand(id, 0), Reshape::Unknown);
        assert_eq!(shrink(0, 0), Reshape::Unknown);
    }

    #[test]
    fn a_hunk_index_out_of_range_changes_nothing() {
        let id = open(BEFORE, AFTER, "src/lib.rs");
        assert_eq!(expand(id, 9), Reshape::Unchanged);
        close(id);
    }
}
