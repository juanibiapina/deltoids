//! Build enriched [`Hunk`]s from planned [`ContextRange`]s and the diff ops.
//!
//! Owns the *filling* phase of the diff engine. Given a list of context
//! ranges already planned by [`super::range`], walk the diff ops and
//! collect the lines that fall inside each range into a [`Hunk`]. Also
//! computes each hunk's ancestor breadcrumb chain by inspecting the
//! anchor lines collected during line collection.
//!
//! Entry point: [`build`].

use super::range::insert_forms_new_scope;
use super::{AncestorSource, ContextRange, DiffLine, Hunk, LineKind, ScopeNode};
use crate::engine::DiffOp;

struct HunkBuildContext<'a> {
    old_parsed: &'a crate::syntax::ParsedFile,
    new_parsed: &'a crate::syntax::ParsedFile,
    old_lines: &'a [&'a str],
    new_lines: &'a [&'a str],
    /// NEW-space inclusive spans claimed by new-scope-span hunks, as
    /// planned by [`super::range::new_replace_scope_ranges`]. The single
    /// source of truth for "which NEW lines belong to a fresh scope's own
    /// hunk"; the aligned-edit added side reads these instead of
    /// re-deriving ownership per op.
    new_scope_spans: &'a [(usize, usize)],
}

#[derive(Default)]
struct HunkBuilder {
    lines: Vec<DiffLine>,
    new_start: Option<usize>,
    anchor_candidates: Vec<(AncestorSource, usize)>,
}

fn collect_equal_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    old_index: usize,
    new_index: usize,
    len: usize,
    old_lines: &[&str],
) {
    for i in 0..len {
        let old_line = old_index + i;
        if old_line < range.start || old_line > range.end {
            continue;
        }
        if builder.new_start.is_none() {
            builder.new_start = Some(new_index + i + 1);
        }
        builder.lines.push(DiffLine {
            kind: LineKind::Context,
            content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
        });
    }
}

fn collect_delete_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    old_index: usize,
    old_len: usize,
    new_index: usize,
    old_lines: &[&str],
) {
    for i in 0..old_len {
        let old_line = old_index + i;
        if old_line < range.start || old_line > range.end {
            continue;
        }
        if builder.new_start.is_none() {
            builder.new_start = Some(new_index + 1);
        }
        builder
            .anchor_candidates
            .push((AncestorSource::Old, old_line));
        builder.lines.push(DiffLine {
            kind: LineKind::Removed,
            content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
        });
    }
}

fn collect_insert_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    old_index: usize,
    new_index: usize,
    new_len: usize,
    ctx: &HunkBuildContext<'_>,
) {
    // Allow inserts at range.end + 1 to handle end-of-file insertions.
    // When inserting at end of file, old_index = total_old, but range.end
    // is clamped to total_old - 1. The insert should still be included.
    if old_index < range.start || old_index > range.end + 1 {
        return;
    }

    // Skip inserts that form a new scope when building an old-scope hunk.
    // The new scope has its own hunk; we don't want to duplicate it as context.
    if range.ancestor_source == AncestorSource::Old
        && insert_forms_new_scope(ctx.new_parsed, new_index, new_index + new_len)
    {
        return;
    }

    if builder.new_start.is_none() {
        builder.new_start = Some(new_index + 1);
    }
    for i in 0..new_len {
        let new_line = new_index + i;
        builder
            .anchor_candidates
            .push((AncestorSource::New, new_line));
        builder.lines.push(DiffLine {
            kind: LineKind::Added,
            content: ctx
                .new_lines
                .get(new_line)
                .copied()
                .unwrap_or("")
                .to_string(),
        });
    }
}

fn collect_replace_removed_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    old_index: usize,
    old_len: usize,
    new_index: usize,
    old_lines: &[&str],
) -> bool {
    let mut added_in_range = false;
    for i in 0..old_len {
        let old_line = old_index + i;
        if old_line < range.start || old_line > range.end {
            continue;
        }
        if builder.new_start.is_none() {
            builder.new_start = Some(new_index + 1);
        }
        builder
            .anchor_candidates
            .push((AncestorSource::Old, old_line));
        builder.lines.push(DiffLine {
            kind: LineKind::Removed,
            content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
        });
        added_in_range = true;
    }
    added_in_range
}

fn trim_trailing_blank_lines(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
}

fn collect_replace_added_lines(
    builder: &mut HunkBuilder,
    new_index: usize,
    new_len: usize,
    ctx: &HunkBuildContext<'_>,
) {
    let mut added_lines = Vec::new();
    for i in 0..new_len {
        let new_line = new_index + i;
        // Skip any NEW line owned by a new-scope-span hunk. The planner
        // records each fresh scope's full span, so a scope whose tail is
        // carried by this op (its start sat in an earlier op) is still
        // recognised and never re-rendered here.
        if ctx
            .new_scope_spans
            .iter()
            .any(|&(start, end)| (start..=end).contains(&new_line))
        {
            continue;
        }
        builder
            .anchor_candidates
            .push((AncestorSource::New, new_line));
        added_lines.push(
            ctx.new_lines
                .get(new_line)
                .copied()
                .unwrap_or("")
                .to_string(),
        );
    }

    trim_trailing_blank_lines(&mut added_lines);
    for content in added_lines {
        builder.lines.push(DiffLine {
            kind: LineKind::Added,
            content,
        });
    }
}

struct ReplaceOpData {
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
}

fn collect_replace_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    replace: ReplaceOpData,
    ctx: &HunkBuildContext<'_>,
) {
    let added_in_range = collect_replace_removed_lines(
        builder,
        range,
        replace.old_index,
        replace.old_len,
        replace.new_index,
        ctx.old_lines,
    );
    if added_in_range {
        collect_replace_added_lines(builder, replace.new_index, replace.new_len, ctx);
    }
}

/// Render a brand-new named scope embedded in a `Replace`'s new content.
///
/// The scope's whole new-file line span (`scope_id`, NEW-space inclusive
/// bounds) is emitted as added lines straight from `new_lines`, with
/// leading/trailing blank lines trimmed. This is independent of op
/// boundaries, so a scope whose lines straddle several `Replace`/`Equal`
/// ops still renders in full instead of fragmenting.
fn build_new_scope_span_hunk(range: &ContextRange, ctx: &HunkBuildContext<'_>) -> Option<Hunk> {
    let (scope_start, scope_end) = range.scope_id?;

    let mut start = scope_start;
    let mut end = scope_end.min(ctx.new_lines.len().saturating_sub(1));
    // Trim leading/trailing blank lines from the span.
    while start < end
        && ctx
            .new_lines
            .get(start)
            .is_some_and(|l| l.trim().is_empty())
    {
        start += 1;
    }
    while end > start && ctx.new_lines.get(end).is_some_and(|l| l.trim().is_empty()) {
        end -= 1;
    }

    let mut builder = HunkBuilder {
        new_start: Some(start + 1),
        ..Default::default()
    };
    for new_line in start..=end {
        builder
            .anchor_candidates
            .push((AncestorSource::New, new_line));
        builder.lines.push(DiffLine {
            kind: LineKind::Added,
            content: ctx
                .new_lines
                .get(new_line)
                .copied()
                .unwrap_or("")
                .to_string(),
        });
    }

    if builder.lines.is_empty() {
        return None;
    }

    let ancestors = select_hunk_ancestors(&builder.anchor_candidates, AncestorSource::New, ctx)
        .unwrap_or_else(|| {
            ancestors_at_line(
                AncestorSource::New,
                range.scope_line,
                ctx.old_parsed,
                ctx.new_parsed,
            )
        });

    Some(Hunk {
        old_start: range.start + 1,
        new_start: builder.new_start.unwrap_or(1),
        lines: builder.lines,
        ancestors,
    })
}

fn build_hunk_from_range(
    ops: &[DiffOp],
    range: &ContextRange,
    ctx: &HunkBuildContext<'_>,
) -> Option<Hunk> {
    if range.render_new_scope_span {
        return build_new_scope_span_hunk(range, ctx);
    }

    let mut builder = HunkBuilder::default();

    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => collect_equal_lines(
                &mut builder,
                range,
                *old_index,
                *new_index,
                *len,
                ctx.old_lines,
            ),
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => collect_delete_lines(
                &mut builder,
                range,
                *old_index,
                *old_len,
                *new_index,
                ctx.old_lines,
            ),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => collect_insert_lines(&mut builder, range, *old_index, *new_index, *new_len, ctx),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => collect_replace_lines(
                &mut builder,
                range,
                ReplaceOpData {
                    old_index: *old_index,
                    old_len: *old_len,
                    new_index: *new_index,
                    new_len: *new_len,
                },
                ctx,
            ),
        }
    }

    if builder.lines.is_empty()
        || builder
            .lines
            .iter()
            .all(|line| line.kind == LineKind::Context)
    {
        return None;
    }

    let ancestors = select_hunk_ancestors(&builder.anchor_candidates, range.ancestor_source, ctx)
        .unwrap_or_else(|| {
            ancestors_at_line(
                range.ancestor_source,
                range.scope_line,
                ctx.old_parsed,
                ctx.new_parsed,
            )
        });

    Some(Hunk {
        old_start: range.start + 1,
        new_start: builder.new_start.unwrap_or(1),
        lines: builder.lines,
        ancestors,
    })
}

/// Build hunks from merged context ranges.
///
/// Each merged range becomes one hunk. Lines are collected from ops
/// that fall within the range, and ancestors are computed from the
/// appropriate tree based on the range's ancestor_source.
#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    ops: &[DiffOp],
    ranges: &[ContextRange],
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    old_lines: &[&str],
    new_lines: &[&str],
) -> Vec<Hunk> {
    // The planner's new-scope-span ranges carry each fresh scope's full
    // NEW-space bounds. Collect them once so the aligned-edit added side
    // can skip every line a new-scope-span hunk owns.
    let new_scope_spans: Vec<(usize, usize)> = ranges
        .iter()
        .filter(|r| r.render_new_scope_span)
        .filter_map(|r| r.scope_id)
        .collect();

    let ctx = HunkBuildContext {
        old_parsed,
        new_parsed,
        old_lines,
        new_lines,
        new_scope_spans: &new_scope_spans,
    };

    ranges
        .iter()
        .filter_map(|range| build_hunk_from_range(ops, range, &ctx))
        .collect()
}

fn select_hunk_ancestors(
    candidates: &[(AncestorSource, usize)],
    preferred_source: AncestorSource,
    ctx: &HunkBuildContext<'_>,
) -> Option<Vec<ScopeNode>> {
    let alternate_source = match preferred_source {
        AncestorSource::Old => AncestorSource::New,
        AncestorSource::New => AncestorSource::Old,
    };

    for source in [preferred_source, alternate_source] {
        let chains: Vec<Vec<ScopeNode>> = candidates
            .iter()
            .filter(|(candidate_source, line)| {
                // Blank lines carry no scope signal; excluding them keeps the
                // LCA anchored on the lines that actually name a structure.
                *candidate_source == source && !candidate_line_is_blank(source, *line, ctx)
            })
            .map(|(candidate_source, line)| {
                ancestors_at_line(*candidate_source, *line, ctx.old_parsed, ctx.new_parsed)
            })
            .collect();

        let ancestors = common_ancestor_prefix(&chains);
        if !ancestors.is_empty() {
            return Some(ancestors);
        }
    }

    None
}

/// Longest common prefix (lowest common ancestor) of the given ancestor
/// chains. Scopes are compared by identity within a single tree, so this
/// must only be called on chains from one source.
fn common_ancestor_prefix(chains: &[Vec<ScopeNode>]) -> Vec<ScopeNode> {
    let Some((first, rest)) = chains.split_first() else {
        return Vec::new();
    };
    let mut len = first.len();
    for chain in rest {
        len = len.min(chain.len());
        let mut i = 0;
        while i < len && same_scope(&first[i], &chain[i]) {
            i += 1;
        }
        len = i;
    }
    first[..len].to_vec()
}

/// True when a candidate line is blank in its source tree.
fn candidate_line_is_blank(
    source: AncestorSource,
    line: usize,
    ctx: &HunkBuildContext<'_>,
) -> bool {
    let lines = match source {
        AncestorSource::Old => ctx.old_lines,
        AncestorSource::New => ctx.new_lines,
    };
    lines.get(line).is_none_or(|l| l.trim().is_empty())
}

/// Compare two scopes by identity within a single parse tree.
fn same_scope(a: &ScopeNode, b: &ScopeNode) -> bool {
    a.kind == b.kind && a.name == b.name && a.start_line == b.start_line && a.end_line == b.end_line
}

fn ancestors_at_line(
    ancestor_source: AncestorSource,
    line: usize,
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
) -> Vec<ScopeNode> {
    // Hunk breadcrumbs show named code structures only. Data containers
    // (JSON/TS objects and arrays, YAML mappings) have no name and would
    // just add noise. Anchor-only callbacks (anonymous arrow functions /
    // function expressions) and block-only call-promoted wrappers
    // (`expect { … }`) are anchors but have no name to display — their
    // call signature is already visible as the first context line of the
    // hunk. `breadcrumb_scopes` applies both drops.
    match ancestor_source {
        AncestorSource::Old => old_parsed.breadcrumb_scopes(line),
        AncestorSource::New => new_parsed.breadcrumb_scopes(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(name: &str) -> ScopeNode {
        ScopeNode {
            kind: "kind".to_string(),
            name: name.to_string(),
            start_line: 1,
            end_line: 2,
            text: String::new(),
        }
    }

    #[test]
    fn common_ancestor_prefix_stops_at_divergence() {
        let a = scope("A");
        let b = scope("B");
        let c = scope("C");

        assert_eq!(
            common_ancestor_prefix(&[vec![a.clone(), b.clone()], vec![a.clone(), c.clone()]]),
            vec![a.clone()]
        );
        assert_eq!(
            common_ancestor_prefix(&[
                vec![a.clone(), b.clone()],
                vec![a.clone(), b.clone(), c.clone()]
            ]),
            vec![a.clone(), b.clone()]
        );
        assert_eq!(
            common_ancestor_prefix(&[vec![a.clone()], vec![b.clone()]]),
            Vec::<ScopeNode>::new()
        );
    }

    #[test]
    fn collect_equal_lines_adds_context_and_sets_new_start() {
        let mut builder = HunkBuilder::default();
        let range = ContextRange {
            start: 1,
            end: 2,
            ancestor_source: AncestorSource::Old,
            scope_line: 1,
            prevent_merge: false,
            scope_id: None,
            render_new_scope_span: false,
        };
        let old_lines = ["zero", "one", "two"];

        collect_equal_lines(&mut builder, &range, 1, 10, 2, &old_lines);

        assert_eq!(builder.new_start, Some(11));
        assert_eq!(builder.lines.len(), 2);
        assert!(
            builder
                .lines
                .iter()
                .all(|line| line.kind == LineKind::Context)
        );
    }
}
