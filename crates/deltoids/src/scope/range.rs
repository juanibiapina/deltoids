//! Plan [`ContextRange`]s for a diff: walk the diff ops, anchor each
//! change on its enclosing scope (or fall back to default 3-line
//! context), then merge overlapping or adjacent ranges.
//!
//! Owns the *planning* phase of the diff engine. Output is consumed
//! by [`super::hunk_builder`] to produce real [`super::Hunk`]s.
//!
//! Entry point: [`plan`].

use super::{AncestorSource, ContextRange, ScopeNode};
use crate::engine::{DiffOp, align_old_to_new};
use crate::syntax::ParsedFile;

const MAX_SCOPE_LINES: usize = 200;
const DEFAULT_CONTEXT: usize = 3;

/// Compute and merge context ranges for the given diff.
///
/// Returns a sorted list of non-overlapping [`ContextRange`]s, one per
/// hunk to be built. Each range carries the anchor scope identity and
/// the tree (old or new) to query for ancestor breadcrumbs.
pub(super) fn plan(
    ops: &[DiffOp],
    old_parsed: &ParsedFile,
    new_parsed: &ParsedFile,
    total_old: usize,
    total_new: usize,
) -> Vec<ContextRange> {
    let ranges = compute_context_ranges(ops, old_parsed, new_parsed, total_old, total_new);
    merge_ranges(ranges)
}

/// Compute default 3-line context range for a change.
fn default_context_range(
    old_start: usize,
    old_end: usize,
    total_old: usize,
    ancestor_source: AncestorSource,
    scope_line: usize,
) -> ContextRange {
    let start = old_start.saturating_sub(DEFAULT_CONTEXT);
    let end = (old_end + DEFAULT_CONTEXT).min(total_old.saturating_sub(1));
    ContextRange {
        start,
        end,
        ancestor_source,
        scope_line,
        prevent_merge: false,
        scope_id: None,
    }
}

struct ScopeRangeContext<'a> {
    old_parsed: &'a crate::syntax::ParsedFile,
    new_parsed: &'a crate::syntax::ParsedFile,
    total_old: usize,
    total_new: usize,
}

/// Pick the hunk-anchor scope for a range of lines.
///
/// Strategy:
/// 1. Prefer the innermost *structure* scope (function, class, method,
///    promoted arrow-field) at the change first line that fits under
///    `MAX_SCOPE_LINES`. The structure does not need to contain the whole
///    change range; if the change extends past it (rare, multi-method
///    edit), the hunk anchors on the structure containing the start.
///    Critically we never climb past this innermost structure to an outer
///    scope (e.g. the enclosing class) just because the inner one does
///    not contain the whole range. Climbing produces hunks that include
///    unrelated sibling methods, which is misleading.
/// 2. If step 1 selected no structure (no enclosing structure exists, or
///    the innermost one exceeds `MAX_SCOPE_LINES`), fall back to the
///    outermost *data* scope (JSON/YAML/TS object or array) that wraps
///    the range and fits under `MAX_SCOPE_LINES`.
/// 3. Otherwise return `None` and let the caller use default 3-line context.
fn scope_for_range(
    parsed: &crate::syntax::ParsedFile,
    range_start: usize,
    range_end: usize,
) -> Option<ScopeNode> {
    let scopes = parsed.enclosing_scopes(range_start);
    let contains_range = |scope: &ScopeNode| {
        let (scope_start, scope_end, _) = scope_bounds(scope);
        scope_start <= range_start && scope_end >= range_end.saturating_sub(1)
    };
    let fits = |scope: &ScopeNode| scope_bounds(scope).2 <= MAX_SCOPE_LINES;

    // 1. Innermost structure that fits. We do NOT require it to contain
    //    the whole range and we do NOT climb to outer structures when the
    //    innermost one does not fit. If the innermost structure exists
    //    but does not fit, fall through to the data tier (rare for code)
    //    or to default context.
    if let Some(s) = scopes.iter().rev().find(|s| parsed.is_structure(s))
        && fits(s)
    {
        return Some(s.clone());
    }

    // 2. Outermost data container that contains the range and fits.
    if let Some(s) = scopes
        .iter()
        .find(|s| parsed.is_data(s) && contains_range(s) && fits(s))
    {
        return Some(s.clone());
    }

    None
}

/// Pick the hunk-anchor scope at a single line.
///
/// Uses the same structure-first, data-fallback strategy as `scope_for_range`.
fn scope_at(parsed: &crate::syntax::ParsedFile, line: usize) -> Option<ScopeNode> {
    scope_for_range(parsed, line, line + 1)
}

/// Innermost named structure (function, class, method, promoted arrow-field)
/// that encloses `line`, regardless of size. Used as the breadcrumb anchor
/// and `scope_id` even when the structure is too large to expand the hunk
/// to its full extent.
fn innermost_structure_at(parsed: &crate::syntax::ParsedFile, line: usize) -> Option<ScopeNode> {
    let scopes = parsed.enclosing_scopes(line);
    scopes.into_iter().rev().find(|s| parsed.is_structure(s))
}

fn scope_bounds(scope: &ScopeNode) -> (usize, usize, usize) {
    let start = scope.start_line.saturating_sub(1);
    let end = scope.end_line.saturating_sub(1);
    let lines = end - start + 1;
    (start, end, lines)
}

fn query_old_line(old_index: usize, total_old: usize) -> usize {
    if old_index < total_old {
        old_index
    } else {
        total_old.saturating_sub(1)
    }
}

fn find_inserted_scope_line(
    parsed: &crate::syntax::ParsedFile,
    new_start: usize,
    new_end: usize,
    total_new: usize,
) -> Option<usize> {
    for line in new_start..new_end.min(total_new) {
        let Some(scope) = scope_at(parsed, line) else {
            continue;
        };
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        let is_new_scope = scope_start >= new_start && scope_end < new_end;
        if scope_lines <= MAX_SCOPE_LINES && is_new_scope {
            return Some(scope_start + 1);
        }
    }
    None
}

/// Check if an insert operation forms a new scope that should have its own hunk.
/// Used to avoid duplicating new scope content in sibling function hunks.
pub(super) fn insert_forms_new_scope(
    parsed: &crate::syntax::ParsedFile,
    new_start: usize,
    new_end: usize,
) -> bool {
    for line in new_start..new_end {
        let Some(scope) = scope_at(parsed, line) else {
            continue;
        };
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        let is_new_scope = scope_start >= new_start && scope_end < new_end;
        if scope_lines <= MAX_SCOPE_LINES && is_new_scope {
            return true;
        }
    }
    false
}

fn find_replace_scope_line(
    parsed: &crate::syntax::ParsedFile,
    new_start: usize,
    new_end: usize,
    total_new: usize,
) -> Option<usize> {
    for line in new_start..new_end.min(total_new) {
        let Some(scope) = scope_at(parsed, line) else {
            continue;
        };
        let (scope_start, _, scope_lines) = scope_bounds(&scope);
        if scope_lines <= MAX_SCOPE_LINES && scope_start >= new_start {
            return Some(scope_start + 1);
        }
    }
    None
}

fn context_ranges_for_insert(
    old_index: usize,
    new_index: usize,
    new_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> Vec<ContextRange> {
    let new_start = new_index;
    let new_end = new_index + new_len;

    if let Some(scope_line) =
        find_inserted_scope_line(ctx.new_parsed, new_start, new_end, ctx.total_new)
    {
        let scope_id = scope_at(ctx.new_parsed, scope_line.saturating_sub(1)).map(|s| {
            let (s0, s1, _) = scope_bounds(&s);
            (s0, s1)
        });
        return vec![ContextRange {
            start: old_index,
            end: old_index,
            ancestor_source: AncestorSource::New,
            scope_line,
            prevent_merge: true,
            scope_id,
        }];
    }

    let scope_line = query_old_line(old_index, ctx.total_old);
    let inner = innermost_structure_at(ctx.old_parsed, scope_line);
    let inner_id = inner.as_ref().map(|s| {
        let (st, en, _) = scope_bounds(s);
        (st, en)
    });
    if let Some(scope) = scope_at(ctx.old_parsed, scope_line) {
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines <= MAX_SCOPE_LINES {
            return vec![ContextRange {
                start: scope_start,
                end: scope_end,
                ancestor_source: AncestorSource::Old,
                scope_line,
                prevent_merge: false,
                scope_id: Some((scope_start, scope_end)),
            }];
        }
    }

    let mut range = default_context_range(
        old_index,
        old_index,
        ctx.total_old,
        AncestorSource::Old,
        old_index,
    );
    range.scope_id = inner_id;
    vec![range]
}

fn context_ranges_for_delete(
    old_index: usize,
    old_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> Vec<ContextRange> {
    let old_start = old_index;
    let old_end = old_index + old_len;

    let inner = innermost_structure_at(ctx.old_parsed, old_start);
    let inner_id = inner.as_ref().map(|s| {
        let (st, en, _) = scope_bounds(s);
        (st, en)
    });

    let mut ranges = Vec::new();
    let mut cursor = old_start;
    let mut last_pushed_end: Option<usize> = None;
    while cursor < old_end.min(ctx.total_old) {
        let Some(scope) = scope_at(ctx.old_parsed, cursor) else {
            cursor += 1;
            continue;
        };
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines > MAX_SCOPE_LINES {
            cursor += 1;
            continue;
        }

        // Adjacent ranges must not overlap. When nothing has been
        // pushed yet, anchor at the start of the delete op (preserves
        // single-scope and run-of-fully-deleted-scopes shapes).
        let range_start = match last_pushed_end {
            Some(prev) => prev + 1,
            None => old_start,
        };

        let is_scope_deleted = scope_start >= old_start && scope_end < old_end;
        if is_scope_deleted {
            let range_end = old_end.saturating_sub(1);
            ranges.push(ContextRange {
                start: range_start,
                end: range_end,
                ancestor_source: AncestorSource::Old,
                scope_line: scope_start,
                prevent_merge: true,
                scope_id: Some((scope_start, scope_end)),
            });
            last_pushed_end = Some(range_end);
            cursor = old_end;
            continue;
        }

        ranges.push(ContextRange {
            start: scope_start,
            end: scope_end,
            ancestor_source: AncestorSource::Old,
            scope_line: cursor,
            prevent_merge: false,
            scope_id: Some((scope_start, scope_end)),
        });
        last_pushed_end = Some(scope_end);
        cursor = scope_end + 1;
    }

    if ranges.is_empty() {
        let mut range = default_context_range(
            old_start,
            old_end.saturating_sub(1),
            ctx.total_old,
            AncestorSource::Old,
            old_start,
        );
        range.scope_id = inner_id;
        ranges.push(range);
    }

    ranges
}

fn old_replace_context_range(
    old_index: usize,
    old_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> ContextRange {
    let old_start = old_index;
    let old_end = old_index + old_len;

    // The breadcrumb / merge identity is always the innermost named
    // structure at the change line, even when we fall back to default
    // context. This keeps adjacent edits inside the same big method in
    // their own merged hunk and prevents accidental merges with
    // neighbouring methods.
    let inner = innermost_structure_at(ctx.old_parsed, old_start);
    let inner_id = inner.as_ref().map(|s| {
        let (st, en, _) = scope_bounds(s);
        (st, en)
    });

    if let Some(scope) = scope_for_range(ctx.old_parsed, old_start, old_end) {
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines <= MAX_SCOPE_LINES {
            // Extend the range to cover the change itself when it sits
            // outside the tree-sitter scope bounds. A method-level
            // decorator is a sibling of `method_definition`, so a change
            // on the decorator line is BEFORE `scope.start`. Without this
            // extension the hunk would have no removed/added lines and
            // get dropped entirely.
            return ContextRange {
                start: scope_start.min(old_start),
                end: scope_end.max(old_end.saturating_sub(1)),
                ancestor_source: AncestorSource::Old,
                scope_line: old_start,
                prevent_merge: false,
                scope_id: Some((scope_start, scope_end)),
            };
        }
    }

    let mut range = default_context_range(
        old_start,
        old_end.saturating_sub(1),
        ctx.total_old,
        AncestorSource::Old,
        old_start,
    );
    range.scope_id = inner_id;
    range
}

/// True when an OLD scope and a NEW scope occupy the same logical slot in
/// the diff, i.e. the OLD scope's start and end lines map through the diff
/// to the NEW scope's start and end lines. Robust against earlier edits
/// that shifted line numbers, unlike absolute position equality.
pub(super) fn same_slot(old_scope: &ScopeNode, new_scope: &ScopeNode, ops: &[DiffOp]) -> bool {
    let (old_start, old_end, _) = scope_bounds(old_scope);
    let (new_start, new_end, _) = scope_bounds(new_scope);
    let Some(mapped_start) = align_old_to_new(old_start, ops) else {
        return false;
    };
    let Some(mapped_end) = align_old_to_new(old_end, ops) else {
        return false;
    };
    mapped_start == new_start && mapped_end == new_end
}

fn new_replace_scope_range(
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ctx: &ScopeRangeContext<'_>,
    ops: &[DiffOp],
) -> Option<ContextRange> {
    let new_start = new_index;
    let new_end = new_index + new_len;

    let scope_line = find_replace_scope_line(ctx.new_parsed, new_start, new_end, ctx.total_new)?;

    let new_scope = scope_at(ctx.new_parsed, scope_line)?;
    let (new_scope_start, new_scope_end, _) = scope_bounds(&new_scope);

    // Same slot: the OLD scope at `old_index` aligns with the NEW scope
    // through the diff. This catches both pure renames and structural
    // conversions (arrow-property -> method) of the same logical member,
    // even when earlier edits in the file shifted line numbers.
    if let Some(old_scope) = scope_at(ctx.old_parsed, old_index)
        && same_slot(&old_scope, &new_scope, ops)
    {
        return None;
    }

    Some(ContextRange {
        start: old_index + old_len,
        end: old_index + old_len,
        ancestor_source: AncestorSource::New,
        scope_line,
        prevent_merge: true,
        scope_id: Some((new_scope_start, new_scope_end)),
    })
}

fn context_ranges_for_replace(
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ctx: &ScopeRangeContext<'_>,
    ops: &[DiffOp],
) -> Vec<ContextRange> {
    let mut ranges = vec![old_replace_context_range(old_index, old_len, ctx)];
    if let Some(range) = new_replace_scope_range(old_index, old_len, new_index, new_len, ctx, ops) {
        ranges.push(range);
    }
    ranges
}

/// Compute context ranges for each change operation.
///
/// For each change, determines:
/// 1. Which tree to query (new tree for inserts, old tree for deletes/replaces)
/// 2. Whether the change is Exact (spans entire scope) or Contained (within scope)
/// 3. Context expansion: Exact uses minimal range, Contained uses scope expansion
fn compute_context_ranges(
    ops: &[DiffOp],
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    total_old: usize,
    total_new: usize,
) -> Vec<ContextRange> {
    let ctx = ScopeRangeContext {
        old_parsed,
        new_parsed,
        total_old,
        total_new,
    };
    let mut ranges = Vec::new();

    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => ranges.extend(context_ranges_for_insert(
                *old_index, *new_index, *new_len, &ctx,
            )),
            DiffOp::Delete {
                old_index, old_len, ..
            } => ranges.extend(context_ranges_for_delete(*old_index, *old_len, &ctx)),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => ranges.extend(context_ranges_for_replace(
                *old_index, *old_len, *new_index, *new_len, &ctx, ops,
            )),
        }
    }

    ranges
}

/// Merge overlapping or adjacent context ranges.
///
/// When ranges merge, keeps the ancestor_source from the first range
/// (typically the one with deletions/existing code takes precedence).
/// Ranges with prevent_merge=true are never merged with other ranges.
fn merge_ranges(mut ranges: Vec<ContextRange>) -> Vec<ContextRange> {
    if ranges.is_empty() {
        return ranges;
    }

    ranges.sort_by_key(|r| r.start);

    let mut merged = vec![ranges[0]];
    for range in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        let can_merge_flags = !last.prevent_merge && !range.prevent_merge;
        // Don't merge across different scopes; otherwise the merged hunk's
        // breadcrumb would describe only one of the two enclosing scopes.
        // Default-context ranges (scope_id = None) absorb into a neighbour
        // when otherwise mergeable.
        let scope_compatible = match (last.scope_id, range.scope_id) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        // Merge if overlapping or adjacent (end + 1 >= start)
        if can_merge_flags && scope_compatible && last.end + 1 >= range.start {
            last.end = last.end.max(range.end);
            // If the absorbing range had no scope_id, take the new one.
            if last.scope_id.is_none() {
                last.scope_id = range.scope_id;
            }
            // Keep ancestor_source from first range (preserves Old over New)
        } else {
            merged.push(*range);
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops_for(original: &str, updated: &str) -> Vec<DiffOp> {
        crate::engine::Snapshot::compute(original, updated)
            .ops()
            .to_vec()
    }

    #[test]
    fn query_old_line_clamps_to_last_old_line() {
        assert_eq!(query_old_line(5, 3), 2);
        assert_eq!(query_old_line(1, 3), 1);
        assert_eq!(query_old_line(0, 0), 0);
    }

    #[test]
    fn insert_context_ranges_use_minimal_new_scope_context() {
        let original = "\
fn existing() {
    let x = 1;
}
";
        let updated = "\
fn existing() {
    let x = 1;
}

fn new_function() {
    let y = 2;
}
";
        let ops = ops_for(original, updated);
        let old_parsed = crate::syntax::ParsedFile::parse("test.rs", original).unwrap();
        let new_parsed = crate::syntax::ParsedFile::parse("test.rs", updated).unwrap();
        let ranges = compute_context_ranges(
            &ops,
            &old_parsed,
            &new_parsed,
            original.lines().count(),
            updated.lines().count(),
        );

        assert!(ranges.iter().any(|range| {
            range.ancestor_source == AncestorSource::New
                && range.prevent_merge
                && range.start == range.end
        }));
    }

    #[test]
    fn delete_spanning_partial_scope_then_full_scope_covers_full_scope() {
        // Hand-crafted Delete op that legitimately spans a partial scope
        // (the tail of `outer`) and a fully-deleted sibling (`deleted`),
        // without relying on a `similar` alignment artifact. Whatever
        // shape the upstream diff library produces, the planner must
        // emit at least one range that covers the fully-deleted scope.
        // If it does not, every line of `fn deleted` is silently
        // dropped from the engine's hunks.
        let original = concat!(
            "fn outer() {\n",       // 0
            "    keep_me();\n",     // 1
            "    drop_me();\n",     // 2
            "    drop_me_too();\n", // 3
            "}\n",                  // 4
            "\n",                   // 5
            "fn deleted() {\n",     // 6
            "    body_line();\n",   // 7
            "}\n",                  // 8
            "\n",                   // 9
            "fn last() {\n",        // 10
            "    last_body();\n",   // 11
            "}\n",                  // 12
        );
        // The updated content is irrelevant for the planner's delete
        // path, which only consults `old_parsed`. We pass a placeholder
        // so the parsed-file accessors used by the planner work.
        let updated = original;

        // Synthetic ops: Equal[0..2] / Delete[2..9] / Equal[9..13].
        // The Delete starts inside `outer` (at line 2) and runs through
        // the whole `deleted` fn (lines 6..8) plus the trailing blank.
        let ops = vec![
            DiffOp::Equal {
                old_index: 0,
                new_index: 0,
                len: 2,
            },
            DiffOp::Delete {
                old_index: 2,
                old_len: 7,
                new_index: 2,
            },
            DiffOp::Equal {
                old_index: 9,
                new_index: 2,
                len: 4,
            },
        ];

        let old_parsed = crate::syntax::ParsedFile::parse("test.rs", original).unwrap();
        let new_parsed = crate::syntax::ParsedFile::parse("test.rs", updated).unwrap();
        let ranges = compute_context_ranges(
            &ops,
            &old_parsed,
            &new_parsed,
            original.lines().count(),
            updated.lines().count(),
        );

        let deleted_fn_line = original
            .lines()
            .position(|line| line == "fn deleted() {")
            .unwrap();
        assert!(
            ranges
                .iter()
                .any(|r| r.start <= deleted_fn_line && r.end >= deleted_fn_line),
            "expected at least one range to cover line {deleted_fn_line} (`fn deleted()`); got: {ranges:?}"
        );
    }
}
