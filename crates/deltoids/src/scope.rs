//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! used by the TUI to display which function a change belongs to.

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use tree_sitter::{Node, Point};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_SCOPE_LINES: usize = 200;
const DEFAULT_CONTEXT: usize = 3;

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Which tree to use for ancestor lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AncestorSource {
    Old,
    New,
}

/// Range of lines to include as context for a hunk (0-indexed, inclusive).
#[derive(Debug, Clone, Copy)]
struct ContextRange {
    start: usize,
    end: usize,
    /// Which tree to use for ancestor lookup
    ancestor_source: AncestorSource,
    /// Representative line for scope lookup (in the appropriate tree)
    scope_line: usize,
    /// If true, this range should not be merged with adjacent ranges
    /// (used for new scopes that should stay separate from siblings)
    prevent_merge: bool,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Added,
    Removed,
    Context,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
    pub ancestors: Vec<ScopeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNode {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

/// A diff enriched with tree-sitter scope information.
///
/// Use `Diff::compute()` to create a diff from original and updated content.
/// The diff provides both raw diff text and structured hunks with
/// ancestor scope chains.
#[derive(Debug, Clone)]
pub struct Diff {
    hunks: Vec<Hunk>,
    text: String,
}

impl Diff {
    /// Compute a diff between original and updated content.
    ///
    /// Parses the file using tree-sitter (if the language is supported) to
    /// populate each hunk's ancestor scope chain. Hunks use scope-expanded
    /// context (up to 50-line scopes). The `text()` method returns standard
    /// 3-line context.
    pub fn compute(original: &str, updated: &str, path: &str) -> Self {
        let text_diff = TextDiff::from_lines(original, updated);

        let old_parsed = crate::syntax::parse_file(path, original);
        let new_parsed = crate::syntax::parse_file(path, updated);

        let hunks = match (&old_parsed, &new_parsed) {
            (Some(old_p), Some(new_p)) => {
                build_hunks_with_scope(&text_diff, old_p, new_p, original, updated)
            }
            _ => build_hunks_from_unified(&text_diff),
        };

        let text = unified_diff_text(&text_diff);

        Diff { hunks, text }
    }

    /// Returns the diff text with standard 3-line context.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the enriched hunks.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }
}

// ---------------------------------------------------------------------------
// Scope-expanded context helpers
// ---------------------------------------------------------------------------

/// Create unified diff text with 3-line context.
fn unified_diff_text(text_diff: &TextDiff<'_, '_, str>) -> String {
    let mut unified = text_diff.unified_diff();
    unified.context_radius(3).header("original", "modified");
    unified.to_string()
}

/// Build hunks with tree-sitter scope expansion.
///
/// Uses scope-expanded context (up to MAX_SCOPE_LINES scopes) and populates ancestor chains.
/// For insertions, queries the new tree; for deletions/replacements, queries the old tree.
fn build_hunks_with_scope(
    text_diff: &TextDiff<'_, '_, str>,
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    original: &str,
    updated: &str,
) -> Vec<Hunk> {
    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = updated.lines().collect();
    let ops: Vec<_> = text_diff.ops().to_vec();

    let context_ranges = compute_context_ranges(
        &ops,
        old_parsed,
        new_parsed,
        original.as_bytes(),
        updated.as_bytes(),
        old_lines.len(),
        new_lines.len(),
    );

    let merged = merge_ranges(context_ranges);

    build_hunks_from_ranges(
        &ops,
        &merged,
        old_parsed,
        new_parsed,
        original.as_bytes(),
        updated.as_bytes(),
        &old_lines,
        &new_lines,
    )
}

/// Build hunks from similar's unified diff when tree-sitter parsing is unavailable.
///
/// Uses similar's built-in 3-line context and produces hunks with empty ancestors.
fn build_hunks_from_unified(text_diff: &TextDiff<'_, '_, str>) -> Vec<Hunk> {
    let mut unified = text_diff.unified_diff();
    unified.context_radius(3);
    unified
        .iter_hunks()
        .map(|hunk| {
            let ops = hunk.ops();
            let old_start = ops.first().map(|op| op.old_range().start + 1).unwrap_or(1);
            let new_start = ops.first().map(|op| op.new_range().start + 1).unwrap_or(1);

            let lines = hunk
                .iter_changes()
                .map(|change| DiffLine {
                    kind: match change.tag() {
                        ChangeTag::Equal => LineKind::Context,
                        ChangeTag::Delete => LineKind::Removed,
                        ChangeTag::Insert => LineKind::Added,
                    },
                    content: change.value().trim_end_matches('\n').to_string(),
                })
                .collect();

            Hunk {
                old_start,
                new_start,
                lines,
                ancestors: Vec::new(),
            }
        })
        .collect()
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
    }
}

struct ScopeRangeContext<'a> {
    old_parsed: &'a crate::syntax::ParsedFile,
    new_parsed: &'a crate::syntax::ParsedFile,
    old_source: &'a [u8],
    new_source: &'a [u8],
    total_old: usize,
    total_new: usize,
}

fn innermost_scope_at(
    parsed: &crate::syntax::ParsedFile,
    source: &[u8],
    line: usize,
) -> Option<ScopeNode> {
    enclosing_scopes(parsed.tree.root_node(), source, line, parsed.scope_kinds)
        .last()
        .cloned()
}

/// Scope kinds that are "leaf containers" - they should not trigger separate hunks
/// when nested inside a larger scope. Used for object properties, config entries, etc.
const LEAF_CONTAINER_KINDS: &[&str] = &[
    "pair",               // JSON, JS/TS/TSX object properties
    "block_mapping_pair", // YAML mappings
];

/// Check if a scope is a "leaf container" that should not trigger its own hunk.
fn is_leaf_container_scope(scope: &ScopeNode) -> bool {
    LEAF_CONTAINER_KINDS.contains(&scope.kind.as_str())
}

/// Get the innermost scope that should trigger hunk splitting.
/// Returns None if the innermost scope is a leaf container (like `pair`) that has a parent.
/// This prevents nested leaf scopes from triggering separate hunks, while still
/// allowing structural scopes (functions, classes) to get their own hunks.
fn hunk_scope_at(
    parsed: &crate::syntax::ParsedFile,
    source: &[u8],
    line: usize,
) -> Option<ScopeNode> {
    let scopes = enclosing_scopes(parsed.tree.root_node(), source, line, parsed.scope_kinds);
    let innermost = scopes.last()?;

    // If the innermost scope is a leaf container and has a parent, skip it
    if is_leaf_container_scope(innermost) && scopes.len() > 1 {
        return None;
    }

    Some(innermost.clone())
}

/// Find the innermost scope that contains an entire range of lines.
/// Used for scope expansion to ensure all changes in a range are included.
/// Falls back to default context if no scope contains the full range.
fn innermost_scope_containing_range(
    parsed: &crate::syntax::ParsedFile,
    source: &[u8],
    range_start: usize,
    range_end: usize,
) -> Option<ScopeNode> {
    // Get scopes at the start of the range (outermost to innermost)
    let scopes = enclosing_scopes(
        parsed.tree.root_node(),
        source,
        range_start,
        parsed.scope_kinds,
    );

    // Find the innermost scope that contains the entire range
    // Iterate from innermost to outermost
    for scope in scopes.iter().rev() {
        let (scope_start, scope_end, _) = scope_bounds(scope);
        if scope_start <= range_start && scope_end >= range_end.saturating_sub(1) {
            return Some(scope.clone());
        }
    }
    None
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
    source: &[u8],
    new_start: usize,
    new_end: usize,
    total_new: usize,
) -> Option<usize> {
    for line in new_start..new_end.min(total_new) {
        // Use hunk_scope_at to avoid leaf container scopes (like pairs inside functions)
        // triggering separate hunks
        let Some(scope) = hunk_scope_at(parsed, source, line) else {
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
/// Only considers hunk scopes to prevent leaf container scopes (like pairs) from
/// triggering separate hunks when inside a larger scope.
fn insert_forms_new_scope(
    parsed: &crate::syntax::ParsedFile,
    source: &[u8],
    new_start: usize,
    new_end: usize,
) -> bool {
    for line in new_start..new_end {
        // Use hunk_scope_at to avoid leaf container scopes triggering separate hunks
        let Some(scope) = hunk_scope_at(parsed, source, line) else {
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
    source: &[u8],
    new_start: usize,
    new_end: usize,
    total_new: usize,
) -> Option<usize> {
    for line in new_start..new_end.min(total_new) {
        // Use hunk_scope_at to avoid leaf container scopes (like pairs inside functions)
        // triggering separate hunks
        let Some(scope) = hunk_scope_at(parsed, source, line) else {
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

    if let Some(scope_line) = find_inserted_scope_line(
        ctx.new_parsed,
        ctx.new_source,
        new_start,
        new_end,
        ctx.total_new,
    ) {
        return vec![ContextRange {
            start: old_index,
            end: old_index,
            ancestor_source: AncestorSource::New,
            scope_line,
            prevent_merge: true,
        }];
    }

    let scope_line = query_old_line(old_index, ctx.total_old);
    if let Some(scope) = innermost_scope_at(ctx.old_parsed, ctx.old_source, scope_line) {
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines <= MAX_SCOPE_LINES {
            return vec![ContextRange {
                start: scope_start,
                end: scope_end,
                ancestor_source: AncestorSource::Old,
                scope_line,
                prevent_merge: false,
            }];
        }
    }

    vec![default_context_range(
        old_index,
        old_index,
        ctx.total_old,
        AncestorSource::Old,
        old_index,
    )]
}

fn context_ranges_for_delete(
    old_index: usize,
    old_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> Vec<ContextRange> {
    let old_start = old_index;
    let old_end = old_index + old_len;

    for line in old_start..old_end.min(ctx.total_old) {
        let Some(scope) = innermost_scope_at(ctx.old_parsed, ctx.old_source, line) else {
            continue;
        };
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines > MAX_SCOPE_LINES {
            continue;
        }

        let is_scope_deleted = scope_start >= old_start && scope_end < old_end;
        if is_scope_deleted {
            return vec![ContextRange {
                start: old_start,
                end: old_end.saturating_sub(1),
                ancestor_source: AncestorSource::Old,
                scope_line: scope_start,
                prevent_merge: true,
            }];
        }

        return vec![ContextRange {
            start: scope_start,
            end: scope_end,
            ancestor_source: AncestorSource::Old,
            scope_line: line,
            prevent_merge: false,
        }];
    }

    vec![default_context_range(
        old_start,
        old_end.saturating_sub(1),
        ctx.total_old,
        AncestorSource::Old,
        old_start,
    )]
}

fn old_replace_context_range(
    old_index: usize,
    old_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> ContextRange {
    let old_start = old_index;
    let old_end = old_index + old_len;

    // Find the innermost scope that contains the entire change range
    if let Some(scope) =
        innermost_scope_containing_range(ctx.old_parsed, ctx.old_source, old_start, old_end)
    {
        let (scope_start, scope_end, scope_lines) = scope_bounds(&scope);
        if scope_lines <= MAX_SCOPE_LINES {
            return ContextRange {
                start: scope_start,
                end: scope_end,
                ancestor_source: AncestorSource::Old,
                scope_line: old_start,
                prevent_merge: false,
            };
        }
    }

    default_context_range(
        old_start,
        old_end.saturating_sub(1),
        ctx.total_old,
        AncestorSource::Old,
        old_start,
    )
}

fn new_replace_scope_range(
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> Option<ContextRange> {
    let new_start = new_index;
    let new_end = new_index + new_len;

    let scope_line = find_replace_scope_line(
        ctx.new_parsed,
        ctx.new_source,
        new_start,
        new_end,
        ctx.total_new,
    )?;

    // Check if this is a renamed scope rather than a new scope.
    // If the old file has a scope at the same position (same line bounds),
    // this is just a rename and should not create a separate hunk.
    let new_scope = innermost_scope_at(ctx.new_parsed, ctx.new_source, scope_line)?;
    let (new_scope_start, new_scope_end, _) = scope_bounds(&new_scope);

    if let Some(old_scope) = innermost_scope_at(ctx.old_parsed, ctx.old_source, old_index) {
        let (old_scope_start, old_scope_end, _) = scope_bounds(&old_scope);
        // Same position means rename, not new scope
        if old_scope_start == new_scope_start && old_scope_end == new_scope_end {
            return None;
        }
    }

    Some(ContextRange {
        start: old_index + old_len,
        end: old_index + old_len,
        ancestor_source: AncestorSource::New,
        scope_line,
        prevent_merge: true,
    })
}

fn context_ranges_for_replace(
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ctx: &ScopeRangeContext<'_>,
) -> Vec<ContextRange> {
    let mut ranges = vec![old_replace_context_range(old_index, old_len, ctx)];
    if let Some(range) = new_replace_scope_range(old_index, old_len, new_index, new_len, ctx) {
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
    ops: &[similar::DiffOp],
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    old_source: &[u8],
    new_source: &[u8],
    total_old: usize,
    total_new: usize,
) -> Vec<ContextRange> {
    let ctx = ScopeRangeContext {
        old_parsed,
        new_parsed,
        old_source,
        new_source,
        total_old,
        total_new,
    };
    let mut ranges = Vec::new();

    for op in ops {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => ranges.extend(context_ranges_for_insert(
                *old_index, *new_index, *new_len, &ctx,
            )),
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => ranges.extend(context_ranges_for_delete(*old_index, *old_len, &ctx)),
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => ranges.extend(context_ranges_for_replace(
                *old_index, *old_len, *new_index, *new_len, &ctx,
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
        // Don't merge if either range has prevent_merge set
        let can_merge = !last.prevent_merge && !range.prevent_merge;
        // Merge if overlapping or adjacent (end + 1 >= start)
        if can_merge && last.end + 1 >= range.start {
            last.end = last.end.max(range.end);
            // Keep ancestor_source from first range (preserves Old over New)
        } else {
            merged.push(*range);
        }
    }

    merged
}

struct HunkBuildContext<'a> {
    old_parsed: &'a crate::syntax::ParsedFile,
    new_parsed: &'a crate::syntax::ParsedFile,
    old_source: &'a [u8],
    new_source: &'a [u8],
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
    if old_index < range.start || old_index > range.end {
        return;
    }

    // Skip inserts that form a new scope when building an old-scope hunk.
    // The new scope has its own hunk; we don't want to duplicate it as context.
    if range.ancestor_source == AncestorSource::Old
        && insert_forms_new_scope(
            ctx.new_parsed,
            ctx.new_source,
            new_index,
            new_index + new_len,
        )
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
) {
    let new_scope_cutoff = old_scope.and_then(|old_scope| {
        first_different_new_scope_start(
            old_scope,
            new_index,
            new_index + new_len,
            ctx.new_parsed,
            ctx.new_source,
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

    let new_scope = enclosing_scopes(
        ctx.new_parsed.tree.root_node(),
        ctx.new_source,
        range.scope_line,
        ctx.new_parsed.scope_kinds,
    )
    .last()
    .cloned();
    let old_scope = enclosing_scopes(
        ctx.old_parsed.tree.root_node(),
        ctx.old_source,
        old_index,
        ctx.old_parsed.scope_kinds,
    )
    .last()
    .cloned();

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
        collect_replace_added_lines(builder, old_scope, replace.new_index, replace.new_len, ctx);
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

    enclosing_scopes(
        ctx.old_parsed.tree.root_node(),
        ctx.old_source,
        range.scope_line,
        ctx.old_parsed.scope_kinds,
    )
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
        ctx.old_source,
        ctx.new_source,
    )
    .unwrap_or_else(|| {
        ancestors_at_line(
            range.ancestor_source,
            range.scope_line,
            ctx.old_parsed,
            ctx.new_parsed,
            ctx.old_source,
            ctx.new_source,
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
fn build_hunks_from_ranges(
    ops: &[similar::DiffOp],
    ranges: &[ContextRange],
    old_parsed: &crate::syntax::ParsedFile,
    new_parsed: &crate::syntax::ParsedFile,
    old_source: &[u8],
    new_source: &[u8],
    old_lines: &[&str],
    new_lines: &[&str],
) -> Vec<Hunk> {
    let ctx = HunkBuildContext {
        old_parsed,
        new_parsed,
        old_source,
        new_source,
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
    old_source: &[u8],
    new_source: &[u8],
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

            let ancestors = ancestors_at_line(
                *candidate_source,
                *line,
                old_parsed,
                new_parsed,
                old_source,
                new_source,
            );

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
    old_source: &[u8],
    new_source: &[u8],
) -> Vec<ScopeNode> {
    match ancestor_source {
        AncestorSource::Old => enclosing_scopes(
            old_parsed.tree.root_node(),
            old_source,
            line,
            old_parsed.scope_kinds,
        ),
        AncestorSource::New => enclosing_scopes(
            new_parsed.tree.root_node(),
            new_source,
            line,
            new_parsed.scope_kinds,
        ),
    }
}

fn first_different_new_scope_start(
    old_scope: &ScopeNode,
    new_start: usize,
    new_end: usize,
    new_parsed: &crate::syntax::ParsedFile,
    new_source: &[u8],
) -> Option<usize> {
    for line in new_start..new_end {
        let new_scopes = enclosing_scopes(
            new_parsed.tree.root_node(),
            new_source,
            line,
            new_parsed.scope_kinds,
        );
        let Some(innermost) = new_scopes.last() else {
            continue;
        };

        let scope_start = innermost.start_line.saturating_sub(1);
        if scope_start < new_start || scope_start >= new_end {
            continue;
        }

        // If name/kind differ but position is the same, it's a rename, not a new scope
        let same_position = innermost.start_line == old_scope.start_line
            && innermost.end_line == old_scope.end_line;
        if same_position {
            continue;
        }

        if innermost.kind != old_scope.kind || innermost.name != old_scope.name {
            return Some(scope_start);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Core algorithm
// ---------------------------------------------------------------------------

/// Find all enclosing scope nodes from outermost to innermost.
/// Returns a vec of `ScopeNode` with outermost first.
fn enclosing_scopes(
    root: Node,
    source: &[u8],
    line: usize,
    scope_kinds: &[&str],
) -> Vec<ScopeNode> {
    let point = point_at_first_non_whitespace(source, line);
    let Some(node) = root.descendant_for_point_range(point, point) else {
        return Vec::new();
    };

    let mut ancestors = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        if scope_kinds.contains(&n.kind()) {
            let start_line = n.start_position().row + 1;
            let end_line = n.end_position().row + 1;
            let name = n
                .child_by_field_name("name")
                .or_else(|| n.child_by_field_name("type"))
                .or_else(|| n.child_by_field_name("key"))
                .and_then(|name_node| name_node.utf8_text(source).ok())
                .unwrap_or("")
                .to_string();
            let text = source_line_raw(source, n.start_position().row).unwrap_or_default();
            ancestors.push(ScopeNode {
                kind: n.kind().to_string(),
                name,
                start_line,
                end_line,
                text,
            });
        }
        current = n.parent();
    }

    ancestors.reverse();
    ancestors
}

fn point_at_first_non_whitespace(source: &[u8], line: usize) -> Point {
    let column = source_line_raw(source, line)
        .map(|text| {
            let trimmed = text.trim_start_matches(|c: char| c.is_whitespace());
            text.len().saturating_sub(trimmed.len())
        })
        .unwrap_or(0);
    Point::new(line, column)
}

/// Return the 0-indexed source line with original indentation preserved.
fn source_line_raw(source: &[u8], line: usize) -> Option<String> {
    let text = std::str::from_utf8(source).ok()?;
    text.lines().nth(line).map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Diff::compute tests
    // -----------------------------------------------------------------------

    #[test]
    fn compute_empty_returns_empty() {
        let diff = Diff::compute("", "", "test.rs");
        assert!(diff.hunks().is_empty());
    }

    #[test]
    fn compute_single_added_line() {
        let original = "line1\n";
        let updated = "line1\nline2\n";
        let diff = Diff::compute(original, updated, "test.txt");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].lines.len(), 2);
        assert_eq!(hunks[0].lines[0].kind, LineKind::Context);
        assert_eq!(hunks[0].lines[0].content, "line1");
        assert_eq!(hunks[0].lines[1].kind, LineKind::Added);
        assert_eq!(hunks[0].lines[1].content, "line2");
    }

    #[test]
    fn compute_populates_ancestors_for_rust() {
        let original = "\
fn compute() {
    let x = 1;
    let y = 2;
}
";
        let updated = original.replace("let x = 1", "let x = 10");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].kind, "function_item");
        assert_eq!(hunks[0].ancestors[0].name, "compute");
    }

    #[test]
    fn compute_nested_scope_impl_and_function() {
        let original = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        let updated = original.replace("x + 1", "x + 2");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.len(), 2);
        assert_eq!(hunks[0].ancestors[0].kind, "impl_item");
        assert_eq!(hunks[0].ancestors[0].name, "Foo");
        assert_eq!(hunks[0].ancestors[1].kind, "function_item");
        assert_eq!(hunks[0].ancestors[1].name, "compute");
    }

    #[test]
    fn compute_unsupported_language_empty_ancestors() {
        let original = "line1\nline2\nline3\n";
        let updated = "line1\nLINE2\nline3\n";
        let diff = Diff::compute(original, updated, "data.xyz");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].ancestors.is_empty());
    }

    #[test]
    fn compute_top_level_code_empty_ancestors() {
        let original = "let x = 1;\nlet y = 2;\n";
        let updated = "let x = 1;\nlet y = 3;\n";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].ancestors.is_empty());
    }

    #[test]
    fn compute_populates_ancestors_for_json() {
        let original = r#"{
  "scripts": {
    "build": "tsc",
    "test": "jest"
  }
}
"#;
        let updated = original.replace("\"jest\"", "\"vitest\"");
        let diff = Diff::compute(original, &updated, "package.json");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Should show "scripts" as ancestor since "test" is nested under it
        assert!(!hunks[0].ancestors.is_empty());
        // The innermost pair is "test", check its name was extracted via "key" field
        let last = hunks[0].ancestors.last().unwrap();
        assert_eq!(last.kind, "pair");
        assert!(last.name.contains("test") || last.name.contains("scripts"));
    }

    #[test]
    fn compute_populates_ancestors_for_typescript_object_properties() {
        // TypeScript config files use nested object literals
        // Changes inside should show object property ancestors
        let original = r#"export default defineConfig({
  env: {
    schema: {
      PUBLIC_KEY: "value1",
    },
  },
});
"#;
        let updated = original.replace("\"value1\"", "\"value2\"");
        let diff = Diff::compute(original, &updated, "astro.config.ts");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Should show nested object properties as ancestors
        assert!(
            !hunks[0].ancestors.is_empty(),
            "TypeScript object properties should produce scope ancestors"
        );
        // Check that we have "pair" ancestors for the nested structure
        let has_pair = hunks[0].ancestors.iter().any(|a| a.kind == "pair");
        assert!(
            has_pair,
            "Should have 'pair' ancestors for object properties"
        );
    }

    #[test]
    fn compute_pair_inside_function_shows_function_ancestor() {
        // When a change is inside an object literal within a function,
        // the function should appear in ancestors (not just the pair)
        let original = r#"function getConfig() {
  return {
    env: {
      key: "value1",
    },
  };
}
"#;
        let updated = original.replace("\"value1\"", "\"value2\"");
        let diff = Diff::compute(original, &updated, "config.ts");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Function should be in ancestors
        let has_function = hunks[0]
            .ancestors
            .iter()
            .any(|a| a.kind == "function_declaration");
        assert!(
            has_function,
            "Function should appear in ancestors when change is inside object within function"
        );
    }

    // -----------------------------------------------------------------------
    // Scope-expanded context tests
    // -----------------------------------------------------------------------

    #[test]
    fn expanded_covers_full_small_function() {
        // A 10-line function (< 50 lines) should have full scope context
        let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
}
";
        let updated = original.replace("let d = 4", "let d = 40");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Hunk should start at line 1 (function start) and include all lines
        assert_eq!(hunks[0].old_start, 1);
        // Should have context lines from the whole function
        let context_count = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Context)
            .count();
        // 10 lines total, 1 changed = 9 context lines
        assert_eq!(context_count, 9, "should have full function as context");
    }

    #[test]
    fn expanded_large_scope_uses_default() {
        // A function > 200 lines should fall back to 3-line context
        let mut lines = vec!["fn big() {".to_string()];
        for i in 1..=205 {
            lines.push(format!("    let x{} = {};", i, i));
        }
        lines.push("}".to_string());
        let original = lines.join("\n") + "\n";
        let updated = original.replace("let x100 = 100", "let x100 = 1000");
        let diff = Diff::compute(&original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Should NOT start at line 1 (function start), should be close to change
        assert!(
            hunks[0].old_start > 1,
            "large scope should use default context"
        );
        // Should have <= 6 context lines (3 before + 3 after)
        let context_count = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Context)
            .count();
        assert!(
            context_count <= 6,
            "large scope should use ~3-line context, got {}",
            context_count
        );
    }

    #[test]
    fn expanded_top_level_uses_default() {
        // Top-level code with no scope should use 3-line default context
        let original = "\
let a = 1;
let b = 2;
let c = 3;
let d = 4;
let e = 5;
let f = 6;
let g = 7;
let h = 8;
let i = 9;
let j = 10;
";
        let updated = original.replace("let e = 5", "let e = 50");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Should not include all 10 lines:
        // 3 before + 1 removed + 1 added + 3 after = 8 lines max
        let total_lines = hunks[0].lines.len();
        assert!(
            total_lines <= 8,
            "top-level should use 3-line context, got {} lines",
            total_lines
        );
        assert!(total_lines < 10, "should not include all lines");
    }

    #[test]
    fn expanded_unsupported_lang_uses_default() {
        // Unknown language should use 3-line default context
        let mut lines: Vec<String> = (1..=20).map(|i| format!("line{}", i)).collect();
        let original = lines.join("\n") + "\n";
        lines[9] = "CHANGED".to_string();
        let updated = lines.join("\n") + "\n";
        let diff = Diff::compute(&original, &updated, "data.xyz");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // 3 before + 1 removed + 1 added + 3 after = 8 lines
        let total_lines = hunks[0].lines.len();
        assert!(
            total_lines <= 8,
            "unsupported lang should use 3-line context, got {} lines",
            total_lines
        );
        assert!(total_lines < 20, "should not include all lines");
    }

    #[test]
    fn expanded_two_functions_separate_hunks() {
        // Two changes in separate functions should produce 2 hunks
        let original = "\
fn first() {
    let a = 1;
}

fn second() {
    let b = 2;
}
";
        let updated = original
            .replace("let a = 1", "let a = 10")
            .replace("let b = 2", "let b = 20");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 2, "two functions should produce 2 hunks");
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "first");
        assert_eq!(hunks[1].ancestors.len(), 1);
        assert_eq!(hunks[1].ancestors[0].name, "second");
    }

    #[test]
    fn expanded_same_function_merges() {
        // Two changes in the same function should produce 1 merged hunk
        let original = "\
fn compute() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
}
";
        let updated = original
            .replace("let a = 1", "let a = 10")
            .replace("let d = 4", "let d = 40");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(
            hunks.len(),
            1,
            "same function changes should merge to 1 hunk"
        );
        // Should have 2 changed lines (a and d)
        let removed = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Removed)
            .count();
        let added = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Added)
            .count();
        assert_eq!(removed, 2, "should have 2 removed lines");
        assert_eq!(added, 2, "should have 2 added lines");
    }

    #[test]
    fn text_returns_standard_3_line_context() {
        // text() should return standard 3-line context diff
        let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
}
";
        let updated = original.replace("let d = 4", "let d = 40");
        let diff = Diff::compute(original, &updated, "test.rs");
        let text = diff.text();
        // Body should NOT include line 1 (fn small()) - only 3-line context
        let lines: Vec<&str> = text.lines().collect();
        let has_fn_line_in_body = lines.iter().any(|l| l.contains("fn small()"));
        assert!(
            !has_fn_line_in_body,
            "body should use 3-line context, not full scope"
        );
    }

    #[test]
    fn hunks_have_expanded_context() {
        // hunks() should have scope-expanded context
        let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
}
";
        let updated = original.replace("let d = 4", "let d = 40");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Hunks should include full function (start at line 1)
        assert_eq!(hunks[0].old_start, 1, "hunk should start at function start");
        // First context line should be fn small() {
        assert!(
            hunks[0].lines[0].content.contains("fn small()"),
            "first context line should be function signature"
        );
    }

    // -----------------------------------------------------------------------
    // enclosing_scopes tests
    // -----------------------------------------------------------------------

    fn parse_and_scopes(source: &str, path: &str, line: usize) -> Vec<ScopeNode> {
        let parsed = crate::syntax::parse_file(path, source).unwrap();
        enclosing_scopes(
            parsed.tree.root_node(),
            source.as_bytes(),
            line,
            parsed.scope_kinds,
        )
    }

    #[test]
    fn enclosing_scopes_returns_full_chain_for_nested_rust() {
        let source = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        // Line 5 is "x + 1" (0-indexed: 5)
        let scopes = parse_and_scopes(source, "test.rs", 5);
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].kind, "impl_item");
        assert_eq!(scopes[0].name, "Foo");
        assert_eq!(scopes[0].start_line, 3);
        assert_eq!(scopes[1].kind, "function_item");
        assert_eq!(scopes[1].name, "compute");
        assert_eq!(scopes[1].start_line, 4);
    }

    #[test]
    fn enclosing_scopes_returns_single_entry_for_top_level_function() {
        let source = "fn hello() {\n    println!(\"hi\");\n}\n";
        let scopes = parse_and_scopes(source, "test.rs", 1);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].kind, "function_item");
        assert_eq!(scopes[0].name, "hello");
    }

    #[test]
    fn enclosing_scopes_returns_empty_for_top_level_code() {
        let source = "let x = 1;\nlet y = 2;\n";
        let scopes = parse_and_scopes(source, "test.rs", 0);
        assert!(scopes.is_empty());
    }

    #[test]
    fn enclosing_scopes_extracts_name_for_various_kinds() {
        // struct
        let source = "struct MyStruct {\n    field: i32,\n}\n";
        let scopes = parse_and_scopes(source, "test.rs", 1);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].kind, "struct_item");
        assert_eq!(scopes[0].name, "MyStruct");

        // enum
        let source = "enum Color {\n    Red,\n}\n";
        let scopes = parse_and_scopes(source, "test.rs", 1);
        assert_eq!(scopes[0].kind, "enum_item");
        assert_eq!(scopes[0].name, "Color");

        // trait
        let source = "trait Drawable {\n    fn draw(&self);\n}\n";
        let scopes = parse_and_scopes(source, "test.rs", 1);
        assert_eq!(scopes[0].kind, "trait_item");
        assert_eq!(scopes[0].name, "Drawable");

        // mod
        let source = "mod utils {\n    fn helper() {}\n}\n";
        let scopes = parse_and_scopes(source, "test.rs", 1);
        assert_eq!(scopes[0].kind, "mod_item");
        assert_eq!(scopes[0].name, "utils");
    }

    #[test]
    fn enclosing_scopes_preserves_original_indentation() {
        let source = "\
impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        let scopes = parse_and_scopes(source, "test.rs", 3);
        assert_eq!(scopes[0].text, "impl Foo {");
        assert_eq!(scopes[1].text, "    fn compute(&self) -> i32 {");
    }

    // -----------------------------------------------------------------------
    // Diff tests
    // -----------------------------------------------------------------------

    #[test]
    fn diff_compute_produces_hunks_and_raw_text() {
        let original = "line1\nline2\n";
        let updated = "line1\nLINE2\n";
        let diff = Diff::compute(original, updated, "test.txt");

        assert_eq!(diff.hunks().len(), 1);
        assert!(diff.text().contains("-line2"));
        assert!(diff.text().contains("+LINE2"));
    }

    #[test]
    fn hunks_nested_shows_innermost_ancestor() {
        let original = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        let updated = original.replace("x + 1", "x + 2");
        let diff = Diff::compute(original, &updated, "test.rs");

        // Innermost ancestor should be function, not impl
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.last().unwrap().name, "compute");
    }

    #[test]
    fn diff_hunks_returns_enriched_data() {
        let original = "fn hello() {\n    println!(\"hi\");\n}\n";
        let updated = "fn hello() {\n    println!(\"bye\");\n}\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "hello");
    }

    // -----------------------------------------------------------------------
    // Line-diff guided scope detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn query_old_line_clamps_to_last_old_line() {
        assert_eq!(query_old_line(5, 3), 2);
        assert_eq!(query_old_line(1, 3), 1);
        assert_eq!(query_old_line(0, 0), 0);
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
        let diff = TextDiff::from_lines(original, updated);
        let ops = diff.ops().to_vec();
        let old_parsed = crate::syntax::parse_file("test.rs", original).unwrap();
        let new_parsed = crate::syntax::parse_file("test.rs", updated).unwrap();
        let ranges = compute_context_ranges(
            &ops,
            &old_parsed,
            &new_parsed,
            original.as_bytes(),
            updated.as_bytes(),
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
    fn new_function_minimal_context() {
        // Adding a complete new function should show only that function,
        // not pull in siblings from the parent scope.
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
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        // Should produce one hunk for the new function
        assert_eq!(hunks.len(), 1);

        // The hunk should be all additions (the new function)
        let added = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Added)
            .count();
        assert_eq!(
            added, 4,
            "should have 4 added lines (blank + fn signature + body + close)"
        );

        // Context should NOT include the existing function
        let has_existing = hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("existing"));
        assert!(
            !has_existing,
            "new function hunk should not include sibling"
        );

        // Ancestors should come from the NEW tree and include the new function
        assert!(!hunks[0].ancestors.is_empty());
        assert_eq!(hunks[0].ancestors[0].name, "new_function");
    }

    #[test]
    fn new_method_in_impl_correct_scope() {
        // Adding a new method to an impl should show method scope, not entire impl.
        let original = "\
struct Foo;

impl Foo {
    fn existing(&self) {
        let x = 1;
    }
}
";
        let updated = "\
struct Foo;

impl Foo {
    fn existing(&self) {
        let x = 1;
    }

    fn new_method(&self) {
        let y = 2;
    }
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        assert_eq!(hunks.len(), 1);

        // Should not include the existing method as context
        let has_existing_body = hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("let x = 1"));
        assert!(
            !has_existing_body,
            "new method should not include sibling method body"
        );

        // Ancestors should include impl and the new method
        assert!(!hunks[0].ancestors.is_empty());
        let has_new_method = hunks[0].ancestors.iter().any(|a| a.name == "new_method");
        assert!(has_new_method, "ancestors should include new_method");
    }

    #[test]
    fn deleted_function_minimal_context() {
        // Deleting a complete function should show only that function.
        let original = "\
fn first() {
    let a = 1;
}

fn to_delete() {
    let b = 2;
}

fn third() {
    let c = 3;
}
";
        let updated = "\
fn first() {
    let a = 1;
}

fn third() {
    let c = 3;
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        assert_eq!(hunks.len(), 1);

        // Should only have the deleted function lines
        let removed = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Removed)
            .count();
        // 5 lines: blank + fn signature + body + close + blank
        assert!(removed >= 4, "should have deleted function lines");

        // Should not include first() or third() body as context
        let has_first_body = hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("let a = 1"));
        let has_third_body = hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("let c = 3"));
        assert!(!has_first_body, "should not include first() body");
        assert!(!has_third_body, "should not include third() body");

        // Ancestors should be from the OLD tree and include to_delete
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "to_delete");
    }

    #[test]
    fn insert_inside_function_expands() {
        // Adding lines inside an existing function should expand to function scope.
        let original = "\
fn compute() {
    let a = 1;
    let c = 3;
}
";
        let updated = "\
fn compute() {
    let a = 1;
    let b = 2;
    let c = 3;
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        assert_eq!(hunks.len(), 1);

        // Should include the function signature as context
        let has_fn_sig = hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("fn compute()"));
        assert!(has_fn_sig, "should expand to include function signature");

        // Should include closing brace as context
        let has_close = hunks[0].lines.iter().any(|l| l.content.trim() == "}");
        assert!(has_close, "should include closing brace");

        // Ancestors should reference the function
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "compute");
    }

    #[test]
    fn multiple_changes_same_function_merge() {
        // Three changes in one function should merge into one hunk.
        let original = "\
fn process() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
}
";
        let updated = original
            .replace("let a = 1", "let a = 10")
            .replace("let c = 3", "let c = 30")
            .replace("let e = 5", "let e = 50");
        let diff = Diff::compute(original, &updated, "test.rs");
        let hunks = diff.hunks();

        assert_eq!(
            hunks.len(),
            1,
            "three changes in same function should produce 1 hunk"
        );

        // All three changes should be in this hunk
        let removed = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Removed)
            .count();
        let added = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Added)
            .count();
        assert_eq!(removed, 3, "should have 3 removed lines");
        assert_eq!(added, 3, "should have 3 added lines");
    }

    #[test]
    fn new_function_and_modified_sibling_separate() {
        // Adding a new function AND modifying a sibling should produce two hunks.
        let original = "\
fn existing() {
    let x = 1;
}
";
        let updated = "\
fn existing() {
    let x = 10;
}

fn new_function() {
    let y = 2;
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        // Should have two hunks: one for modification, one for addition
        assert_eq!(hunks.len(), 2, "modify + add should produce 2 hunks");

        // First hunk should be the modification to existing()
        assert!(hunks[0].ancestors.iter().any(|a| a.name == "existing"));

        // Second hunk should be the new function
        assert!(hunks[1].ancestors.iter().any(|a| a.name == "new_function"));
    }

    #[test]
    fn replace_with_add_and_delete_same_function_single_hunk() {
        // Bug: When a Replace operation adds some lines and removes others
        // within the same function, it was creating duplicate hunks with
        // the same scope header.
        //
        // This reproduces the bug seen in `git show | deltoids` where
        // process_diff appeared twice.
        let original = "\
fn process() {
    let a = 1;
    // old comment
    let b = 2;
    let c = 3;
}
";
        // Change: remove "old comment", add "new comment" in different place
        let updated = "\
fn process() {
    let a = 1;
    // new comment
    let b = 2;
    let c = 3;
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        // Should produce exactly ONE hunk, not two
        assert_eq!(
            hunks.len(),
            1,
            "modifications within same function should produce 1 hunk, got {}",
            hunks.len()
        );

        // The single hunk should have the function as ancestor
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "process");
    }

    #[test]
    fn two_distant_changes_same_function_no_duplicate_headers() {
        // Bug reproduction: Two separate changes in the same function,
        // far enough apart to be separate git hunks, but within the same
        // scope. The second change has both additions and deletions.
        //
        // This matches the pattern in `git show HEAD` for main.rs where
        // process_diff header appeared twice.
        let original = "\
fn process_diff() {
    // Section 1
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    
    // Section 2
    let i = 9;
    // old comment to remove
    let j = 10;
    let k = 11;
}
";
        let updated = "\
fn process_diff() {
    // Section 1
    let a = 1;
    // NEW: added line in section 1
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    
    // Section 2
    let i = 9;
    let j = 10;
    // NEW: added line in section 2
    let k = 11;
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        // Should produce exactly ONE hunk (both changes are in same function scope)
        assert_eq!(
            hunks.len(),
            1,
            "two changes in same function should merge to 1 hunk, got {}",
            hunks.len()
        );

        // Verify the hunk has the correct ancestor
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "process_diff");
    }

    #[test]
    fn rename_function_produces_single_hunk() {
        // Bug: When renaming a function (same location, different name),
        // the code treats the new function as a "new scope" and creates
        // two separate hunks instead of one merged hunk.
        //
        // This reproduces the bug seen in edit trace entry 5 (render.rs)
        // where `fn theme()` was renamed to `fn syntect_theme()` and
        // produced two separate hunks with different ancestors.
        let original = "\
fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_theme(THEME_NAME)
            .clone()
    })
}
";
        let updated = "\
fn syntect_theme() -> &'static SyntectTheme {
    SYNTECT_THEME.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_theme(THEME_NAME)
            .clone()
    })
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        // Bug: Currently produces 2 hunks when it should produce 1.
        // First hunk has ancestor "theme" (old), second has "syntect_theme" (new).
        assert_eq!(
            hunks.len(),
            1,
            "renaming a function should produce 1 hunk, not {}; \
             hunks have ancestors: {:?}",
            hunks.len(),
            hunks.iter().map(|h| &h.ancestors).collect::<Vec<_>>()
        );

        // The single hunk should have changes (not be context-only)
        let has_removed = hunks[0].lines.iter().any(|l| l.kind == LineKind::Removed);
        let has_added = hunks[0].lines.iter().any(|l| l.kind == LineKind::Added);
        assert!(has_removed, "hunk should have removed lines");
        assert!(has_added, "hunk should have added lines");

        // The ancestor should be from one of the functions (either old or new is fine)
        assert!(!hunks[0].ancestors.is_empty(), "hunk should have ancestors");
        let ancestor_name = &hunks[0].ancestors[0].name;
        assert!(
            ancestor_name == "theme" || ancestor_name == "syntect_theme",
            "ancestor should be either 'theme' or 'syntect_theme', got '{}'",
            ancestor_name
        );
    }

    #[test]
    fn replace_function_signature_does_not_create_context_only_hunk() {
        let original = "\
fn default_context_range(old_start: usize, old_end: usize, total_old: usize) -> ContextRange {
    let start = old_start.saturating_sub(DEFAULT_CONTEXT);
    let end = (old_end + DEFAULT_CONTEXT).min(total_old.saturating_sub(1));
    ContextRange { start, end }
}
";
        let updated = "\
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
    }
}
";
        let diff = Diff::compute(original, updated, "test.rs");
        let hunks = diff.hunks();

        assert_eq!(
            hunks.len(),
            1,
            "should not create a context-only sibling hunk"
        );
        assert!(hunks[0].lines.iter().any(|l| l.kind != LineKind::Context));
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].name, "default_context_range");
    }
}
