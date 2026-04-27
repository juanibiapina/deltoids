//! Build enriched [`Hunk`]s from planned [`ContextRange`]s and the diff ops.
//!
//! Owns the *filling* phase of the diff engine. Given a list of context
//! ranges already planned by [`super::range`], walk the diff ops and
//! collect the lines that fall inside each range into a [`Hunk`]. Also
//! computes each hunk's ancestor breadcrumb chain by inspecting the
//! anchor lines collected during line collection.
//!
//! Entry point: [`build`].

use super::{
    AncestorSource, ContextRange, DiffLine, Hunk, LineKind, ScopeNode, insert_forms_new_scope,
    same_slot,
};

struct HunkBuildContext<'a> {
    old_parsed: &'a crate::syntax::ParsedFile,
    new_parsed: &'a crate::syntax::ParsedFile,
    old_lines: &'a [&'a str],
    new_lines: &'a [&'a str],
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
    old_scope: Option<&ScopeNode>,
    new_index: usize,
    new_len: usize,
    ctx: &HunkBuildContext<'_>,
    ops: &[similar::DiffOp],
) {
    let new_scope_cutoff = old_scope.and_then(|old_scope| {
        first_different_new_scope_start(
            old_scope,
            new_index,
            new_index + new_len,
            ctx.new_parsed,
            ops,
        )
    });

    let mut added_lines = Vec::new();
    for i in 0..new_len {
        let new_line = new_index + i;
        if new_scope_cutoff.is_some_and(|cutoff| new_line >= cutoff) {
            break;
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

fn collect_replace_new_scope_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    old_index: usize,
    new_index: usize,
    new_len: usize,
    ctx: &HunkBuildContext<'_>,
) {
    if range.ancestor_source != AncestorSource::New || range.start != old_index {
        return;
    }

    let new_scope = ctx
        .new_parsed
        .enclosing_scopes(range.scope_line)
        .last()
        .cloned();
    let old_scope = ctx.old_parsed.enclosing_scopes(old_index).last().cloned();

    let Some(new_scope) = new_scope else {
        return;
    };
    let is_same_scope = old_scope.as_ref().is_some_and(|old_scope| {
        old_scope.kind == new_scope.kind && old_scope.name == new_scope.name
    });
    if is_same_scope {
        return;
    }

    let scope_start = new_scope.start_line.saturating_sub(1);
    let scope_end = new_scope.end_line.saturating_sub(1);
    for i in 0..new_len {
        let new_line = new_index + i;
        if new_line > scope_end {
            break;
        }

        let content = ctx
            .new_lines
            .get(new_line)
            .copied()
            .unwrap_or("")
            .to_string();
        let include = new_line >= scope_start || content.trim().is_empty();
        if !include {
            continue;
        }

        builder
            .anchor_candidates
            .push((AncestorSource::New, new_line));
        builder.lines.push(DiffLine {
            kind: LineKind::Added,
            content,
        });
    }
}

fn collect_replace_lines(
    builder: &mut HunkBuilder,
    range: &ContextRange,
    replace: ReplaceOpData,
    old_scope: Option<&ScopeNode>,
    ctx: &HunkBuildContext<'_>,
    ops: &[similar::DiffOp],
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
        collect_replace_added_lines(
            builder,
            old_scope,
            replace.new_index,
            replace.new_len,
            ctx,
            ops,
        );
        return;
    }

    collect_replace_new_scope_lines(
        builder,
        range,
        replace.old_index + replace.old_len,
        replace.new_index,
        replace.new_len,
        ctx,
    );
}

fn old_scope_for_range(range: &ContextRange, ctx: &HunkBuildContext<'_>) -> Option<ScopeNode> {
    if range.ancestor_source != AncestorSource::Old {
        return None;
    }

    ctx.old_parsed
        .enclosing_scopes(range.scope_line)
        .last()
        .cloned()
}

fn build_hunk_from_range(
    ops: &[similar::DiffOp],
    range: &ContextRange,
    ctx: &HunkBuildContext<'_>,
) -> Option<Hunk> {
    let mut builder = HunkBuilder::default();
    let old_scope = old_scope_for_range(range, ctx);

    for op in ops {
        match op {
            similar::DiffOp::Equal {
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
            similar::DiffOp::Delete {
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
            similar::DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => collect_insert_lines(&mut builder, range, *old_index, *new_index, *new_len, ctx),
            similar::DiffOp::Replace {
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
                old_scope.as_ref(),
                ctx,
                ops,
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

    let ancestors = select_hunk_ancestors(
        &builder.anchor_candidates,
        range.ancestor_source,
        ctx.old_parsed,
        ctx.new_parsed,
    )
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
    ops: &[similar::DiffOp],
    ranges: &[ContextRange],
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    old_lines: &[&str],
    new_lines: &[&str],
) -> Vec<Hunk> {
    let ctx = HunkBuildContext {
        old_parsed,
        new_parsed,
        old_lines,
        new_lines,
    };

    ranges
        .iter()
        .filter_map(|range| build_hunk_from_range(ops, range, &ctx))
        .collect()
}

fn select_hunk_ancestors(
    candidates: &[(AncestorSource, usize)],
    preferred_source: AncestorSource,
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
) -> Option<Vec<ScopeNode>> {
    let alternate_source = match preferred_source {
        AncestorSource::Old => AncestorSource::New,
        AncestorSource::New => AncestorSource::Old,
    };

    for source in [preferred_source, alternate_source] {
        let mut best_ancestors = None;

        for (candidate_source, line) in candidates {
            if *candidate_source != source {
                continue;
            }

            let ancestors = ancestors_at_line(*candidate_source, *line, old_parsed, new_parsed);

            if ancestors.len() > best_ancestors.as_ref().map_or(0, Vec::len) {
                best_ancestors = Some(ancestors);
            }
        }

        if let Some(ancestors) = best_ancestors
            && !ancestors.is_empty()
        {
            return Some(ancestors);
        }
    }

    None
}

fn ancestors_at_line(
    ancestor_source: AncestorSource,
    line: usize,
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
) -> Vec<ScopeNode> {
    let (parsed, scopes) = match ancestor_source {
        AncestorSource::Old => (old_parsed, old_parsed.enclosing_scopes(line)),
        AncestorSource::New => (new_parsed, new_parsed.enclosing_scopes(line)),
    };
    // Hunk breadcrumbs show named code structures only. Data containers
    // (JSON/TS objects and arrays, YAML mappings) have no name and would
    // just add noise.
    scopes
        .into_iter()
        .filter(|s| parsed.is_structure(s))
        .collect()
}

fn first_different_new_scope_start(
    old_scope: &ScopeNode,
    new_start: usize,
    new_end: usize,
    new_parsed: &crate::syntax::ParsedFile,
    ops: &[similar::DiffOp],
) -> Option<usize> {
    for line in new_start..new_end {
        let new_scopes = new_parsed.enclosing_scopes(line);
        let Some(innermost) = new_scopes.last() else {
            continue;
        };

        // Data-tier scopes (objects, arrays, JSON pairs) never get their own
        // hunk, so they must not cut off the enclosing hunk either.
        if new_parsed.is_data(innermost) {
            continue;
        }

        let scope_start = innermost.start_line.saturating_sub(1);
        if scope_start < new_start || scope_start >= new_end {
            continue;
        }

        // Same logical slot in the diff alignment? Then it's a rename or a
        // structural conversion of the same member, not a brand-new scope.
        if same_slot(old_scope, innermost, ops) {
            continue;
        }

        if innermost.kind != old_scope.kind || innermost.name != old_scope.name {
            return Some(scope_start);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
