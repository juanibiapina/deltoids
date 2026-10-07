//! How much context each change gets: the hunk expansion rules.
//!
//! [`Expansion::window`] turns one [`Change`] into a [`Window`]: the old
//! lines its hunk wants to show, the scope that context anchors on, and
//! whether the change is a whole new or deleted scope that gets a hunk of
//! its own. Windows only size context. Which changed lines a hunk shows is
//! decided by [`super::hunks`], which shows each one exactly once.

use std::ops::Range;

use super::ScopeNode;
use crate::syntax::ParsedFile;

/// Largest structure shown whole as a hunk's context.
const MAX_SCOPE_LINES: usize = 200;
/// Context on each side of a change inside a structure larger than
/// [`MAX_SCOPE_LINES`], clamped to the structure.
const STRUCTURE_CONTEXT: usize = 100;
/// Context on each side of a change with no scope to anchor on.
const DEFAULT_CONTEXT: usize = 3;

/// Which file a line number refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Old,
    New,
}

/// One change: old lines `old` are removed and new lines `new` are added.
/// An insert has an empty `old` at the insertion point; a delete has an
/// empty `new` at the deletion point.
#[derive(Debug, Clone)]
pub(super) struct Change {
    pub(super) old: Range<usize>,
    pub(super) new: Range<usize>,
}

/// The context one change asks for.
#[derive(Debug, Clone)]
pub(super) struct Window {
    /// Old lines to show around the change. Always contains `change.old`,
    /// or its insertion point for an insert.
    pub(super) context: Range<usize>,
    /// Line span of the scope the context anchors on. Windows that only
    /// touch stay apart when they anchor on different scopes.
    pub(super) scope: Option<Range<usize>>,
    /// Line whose breadcrumb names the hunk when its changed lines share
    /// no scope. `None` for whole new or deleted scopes: when those share
    /// no scope they sit at the top level, and an empty breadcrumb says so.
    pub(super) fallback: Option<(Side, usize)>,
    /// A whole new or deleted scope: its own hunk, with no context.
    pub(super) isolated: bool,
}

/// The context policy of one diff.
#[derive(Clone, Copy)]
pub(super) enum Expansion<'a> {
    /// No syntax: [`DEFAULT_CONTEXT`] lines around every change.
    Plain,
    /// Context grows to the scopes of the parsed old and new files.
    Scoped {
        old: &'a ParsedFile,
        new: &'a ParsedFile,
    },
}

impl Expansion<'_> {
    pub(super) fn window(&self, change: &Change, total_old: usize) -> Window {
        match *self {
            Expansion::Plain => default_window(change, total_old),
            Expansion::Scoped { old, new } => scoped_window(change, old, new, total_old),
        }
    }

    /// Breadcrumb chain at `line` in `side`'s file, outermost first.
    pub(super) fn breadcrumb(&self, side: Side, line: usize) -> Vec<ScopeNode> {
        match (*self, side) {
            (Expansion::Plain, _) => Vec::new(),
            (Expansion::Scoped { old, .. }, Side::Old) => old.breadcrumb_scopes(line),
            (Expansion::Scoped { new, .. }, Side::New) => new.breadcrumb_scopes(line),
        }
    }
}

fn default_window(change: &Change, total_old: usize) -> Window {
    Window {
        context: change.old.start.saturating_sub(DEFAULT_CONTEXT)
            ..(change.old.end + DEFAULT_CONTEXT).min(total_old),
        scope: None,
        fallback: None,
        isolated: false,
    }
}

fn scoped_window(change: &Change, old: &ParsedFile, new: &ParsedFile, total_old: usize) -> Window {
    if change.old.is_empty()
        && let Some(scope) = whole_scopes(new, change.new.clone())
    {
        return isolated(change, scope);
    }
    if change.new.is_empty()
        && let Some(scope) = whole_scopes(old, change.old.clone())
    {
        return isolated(change, scope);
    }

    let line = change.old.start.min(total_old.saturating_sub(1));
    let fallback = Some((Side::Old, line));
    if let Some(structure) = old.innermost_structure(line) {
        let scope = structure.lines();
        let context = if scope.len() <= MAX_SCOPE_LINES {
            scope.clone()
        } else {
            change
                .old
                .start
                .saturating_sub(STRUCTURE_CONTEXT)
                .max(scope.start)..(change.old.end + STRUCTURE_CONTEXT).min(scope.end)
        };
        return Window {
            context: cover(context, change),
            scope: Some(scope),
            fallback,
            isolated: false,
        };
    }

    let anchor_lines = if change.old.is_empty() {
        line..line + 1
    } else {
        change.old.clone()
    };
    if let Some(anchor) =
        old.expansion_anchor(anchor_lines.start, anchor_lines.end, MAX_SCOPE_LINES)
    {
        let scope = anchor.lines();
        return Window {
            context: cover(scope.clone(), change),
            scope: Some(scope),
            fallback,
            isolated: false,
        };
    }

    Window {
        fallback,
        ..default_window(change, total_old)
    }
}

fn isolated(change: &Change, scope: Range<usize>) -> Window {
    Window {
        context: change.old.clone(),
        fallback: None,
        scope: Some(scope),
        isolated: true,
    }
}

/// Grow `context` to contain the change's old lines or insertion point.
fn cover(context: Range<usize>, change: &Change) -> Range<usize> {
    context.start.min(change.old.start)..context.end.max(change.old.end)
}

/// The first whole structure `lines` holds, when `lines` consist only of
/// whole structures and the lines between them.
///
/// Every structure boundary (first or last line) inside `lines` must
/// belong to a structure that lies wholly inside them. Lines that carry
/// the closing line of a struct opened before them, for example, are an
/// edit to that struct even when they also hold a whole new function. A
/// structure enclosing all of `lines` has no boundary inside them, and a
/// boundary shared with an inner whole structure (a Python class ending on
/// its appended method's last line) does not count.
fn whole_scopes(parsed: &ParsedFile, lines: Range<usize>) -> Option<Range<usize>> {
    let lines = lines.start..lines.end.min(parsed.line_count());
    let mut inner: Vec<Range<usize>> = Vec::new();
    let mut outer: Vec<Range<usize>> = Vec::new();
    for line in lines.clone() {
        for scope in parsed.enclosing_scopes(line) {
            let scope = scope.lines();
            let side = if lines.start <= scope.start && scope.end <= lines.end {
                &mut inner
            } else {
                &mut outer
            };
            if !side.contains(&scope) {
                side.push(scope);
            }
        }
    }
    let straddles = outer.iter().any(|scope| {
        let last = scope.end - 1;
        (lines.contains(&scope.start) && !inner.iter().any(|s| s.start == scope.start))
            || (lines.contains(&last) && !inner.iter().any(|s| s.end == scope.end))
    });
    if straddles {
        return None;
    }
    lines.clone().find_map(|line| {
        let scope = parsed.innermost_structure(line)?.lines();
        (scope.len() <= MAX_SCOPE_LINES && lines.start <= scope.start && scope.end <= lines.end)
            .then_some(scope)
    })
}
