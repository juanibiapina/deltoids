//! Build [`Hunk`]s from the diff op stream.
//!
//! Every non-`Equal` op becomes one [`Change`], and [`Expansion::window`]
//! sizes its context. Changes are grouped in op order: a change joins the
//! current group when their windows overlap, or touch while anchored on
//! the same scope, unless either is a whole new or deleted scope. Each
//! group's context is clipped so it never reaches into a neighbouring
//! hunk, then the group is emitted as one hunk. A change belongs to
//! exactly one group, so every removed and added line is shown exactly
//! once and hunks never overlap.
//!
//! A widened range (see [`crate::Diff::expand`]) grows the context of every
//! change inside it to the whole range and ends its isolation, so the
//! overlap rule above merges the hunks it covers.

use std::ops::Range;

use super::expansion::{Change, Expansion, Side, Window};
use super::{DiffLine, Hunk, LineKind, ScopeNode};
use crate::engine::DiffOp;

/// Consecutive changes `changes` shown as one hunk with `context` old lines.
struct Group {
    changes: Range<usize>,
    context: Range<usize>,
    scope: Option<Range<usize>>,
    fallback: Option<(Side, usize)>,
    isolated: bool,
}

pub(super) fn build(
    ops: &[DiffOp],
    old_lines: &[&str],
    new_lines: &[&str],
    expansion: &Expansion<'_>,
    widened: &[Range<usize>],
) -> Vec<Hunk> {
    let changes: Vec<Change> = ops.iter().filter_map(change).collect();
    let groups = group(&changes, expansion, widened, old_lines.len());

    let mut hunks = Vec::with_capacity(groups.len());
    let mut old_floor = 0;
    for (index, group) in groups.iter().enumerate() {
        let first = &changes[group.changes.start];
        let ceiling = groups.get(index + 1).map_or(old_lines.len(), |next| {
            changes[next.changes.start].old.start
        });
        let context = group.context.start.max(old_floor)..group.context.end.min(ceiling);
        let members = &changes[group.changes.clone()];
        hunks.push(Hunk {
            old_start: context.start + 1,
            new_start: first.new.start - (first.old.start - context.start) + 1,
            lines: lines(members, context.clone(), old_lines, new_lines),
            ancestors: ancestors(members, group, old_lines, new_lines, expansion),
        });
        old_floor = context.end;
    }
    hunks
}

fn change(op: &DiffOp) -> Option<Change> {
    match *op {
        DiffOp::Equal { .. } => None,
        DiffOp::Insert {
            old_index,
            new_index,
            new_len,
        } => Some(Change {
            old: old_index..old_index,
            new: new_index..new_index + new_len,
        }),
        DiffOp::Delete {
            old_index,
            old_len,
            new_index,
        } => Some(Change {
            old: old_index..old_index + old_len,
            new: new_index..new_index,
        }),
        DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } => Some(Change {
            old: old_index..old_index + old_len,
            new: new_index..new_index + new_len,
        }),
    }
}

fn group(
    changes: &[Change],
    expansion: &Expansion<'_>,
    widened: &[Range<usize>],
    total_old: usize,
) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        let window = widen(expansion.window(change, total_old), change, widened);
        match groups.last_mut() {
            Some(last) if joins(last, &window) => {
                last.changes.end = index + 1;
                last.context = last.context.start.min(window.context.start)
                    ..last.context.end.max(window.context.end);
                last.scope = last.scope.take().or(window.scope);
            }
            _ => groups.push(Group {
                changes: index..index + 1,
                context: window.context,
                scope: window.scope,
                fallback: window.fallback,
                isolated: window.isolated,
            }),
        }
    }
    groups
}

/// Grow `window` to every widened range holding `change`.
fn widen(mut window: Window, change: &Change, widened: &[Range<usize>]) -> Window {
    for range in widened {
        if range.start <= change.old.start && change.old.end <= range.end {
            window.context =
                window.context.start.min(range.start)..window.context.end.max(range.end);
            window.isolated = false;
        }
    }
    window
}

fn joins(group: &Group, window: &Window) -> bool {
    if group.isolated || window.isolated {
        return false;
    }
    let same_scope = match (&group.scope, &window.scope) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    group.context.end > window.context.start
        || (group.context.end == window.context.start && same_scope)
}

fn lines(
    changes: &[Change],
    context: Range<usize>,
    old_lines: &[&str],
    new_lines: &[&str],
) -> Vec<DiffLine> {
    let line = |kind: LineKind, text: &str| DiffLine {
        kind,
        content: text.to_string(),
    };
    let mut lines = Vec::new();
    let mut old_at = context.start;
    for change in changes {
        lines.extend(
            old_lines[old_at..change.old.start]
                .iter()
                .map(|text| line(LineKind::Context, text)),
        );
        lines.extend(
            old_lines[change.old.clone()]
                .iter()
                .map(|text| line(LineKind::Removed, text)),
        );
        lines.extend(
            new_lines[change.new.clone()]
                .iter()
                .map(|text| line(LineKind::Added, text)),
        );
        old_at = change.old.end;
    }
    lines.extend(
        old_lines[old_at..context.end]
            .iter()
            .map(|text| line(LineKind::Context, text)),
    );
    lines
}

/// Breadcrumb = the lowest common ancestor of the hunk's changed lines,
/// in the new file (where the change lands). When the added lines span
/// several sibling scopes the LCA is their shared parent. Pure deletions
/// use the old file, so a deleted scope still names itself. Blank lines
/// carry no scope signal and are skipped. When the changed lines share no
/// scope, the group's fallback line, if any, names the hunk.
fn ancestors(
    changes: &[Change],
    group: &Group,
    old_lines: &[&str],
    new_lines: &[&str],
    expansion: &Expansion<'_>,
) -> Vec<ScopeNode> {
    let added = changes.iter().flat_map(|change| change.new.clone());
    let removed = changes.iter().flat_map(|change| change.old.clone());
    let new_lca = common_ancestors(added, Side::New, new_lines, expansion);
    if !new_lca.is_empty() {
        return new_lca;
    }
    let old_lca = common_ancestors(removed, Side::Old, old_lines, expansion);
    if !old_lca.is_empty() {
        return old_lca;
    }
    group
        .fallback
        .map_or_else(Vec::new, |(side, line)| expansion.breadcrumb(side, line))
}

fn common_ancestors(
    lines: impl Iterator<Item = usize>,
    side: Side,
    text: &[&str],
    expansion: &Expansion<'_>,
) -> Vec<ScopeNode> {
    let mut common: Option<Vec<ScopeNode>> = None;
    for line in lines.filter(|&line| !text[line].trim().is_empty()) {
        let chain = expansion.breadcrumb(side, line);
        common = Some(match common {
            None => chain,
            Some(prefix) => prefix
                .into_iter()
                .zip(chain)
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
        if common.as_ref().is_some_and(Vec::is_empty) {
            break;
        }
    }
    common.unwrap_or_default()
}
