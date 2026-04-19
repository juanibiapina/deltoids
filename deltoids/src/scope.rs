//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! used by the TUI to display which function a change belongs to.

use serde::{Deserialize, Serialize};
use similar::TextDiff;
use tree_sitter::{Node, Point};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_SCOPE_LINES: usize = 50;
const DEFAULT_CONTEXT: usize = 3;

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Range of lines to include as context for a hunk (0-indexed, inclusive).
#[derive(Debug, Clone, Copy)]
struct ContextRange {
    start: usize,
    end: usize,
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
    /// context (up to 50-line scopes), while `text()` returns standard
    /// 3-line context for agent consumption.
    pub fn compute(original: &str, updated: &str, path: &str) -> Self {
        let text_diff = TextDiff::from_lines(original, updated);

        // Standard 3-line context for agent-facing diff
        let mut unified = text_diff.unified_diff();
        unified.context_radius(3).header("original", "modified");
        let text = unified.to_string();

        // Parse for tree-sitter scope expansion
        let parsed = crate::syntax::parse_file(path, original);
        let old_lines: Vec<&str> = original.lines().collect();
        let new_lines: Vec<&str> = updated.lines().collect();
        let ops: Vec<_> = text_diff.ops().to_vec();

        // Compute context ranges (scope-expanded where applicable)
        let context_ranges = compute_context_ranges(
            &ops,
            parsed.as_ref(),
            original.as_bytes(),
            old_lines.len(),
        );

        // Merge overlapping/adjacent ranges
        let merged = merge_ranges(context_ranges);

        // Build hunks from merged ranges
        let hunks = build_hunks_from_ranges(
            &ops,
            &merged,
            parsed.as_ref(),
            original.as_bytes(),
            &old_lines,
            &new_lines,
        );

        Diff { hunks, text }
    }

    /// Returns the diff text with standard 3-line context.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns diff text with scope context injected into @@ headers.
    ///
    /// Each hunk header is appended with the innermost ancestor's source line
    /// (trimmed), making it easier to see which function/struct a change belongs to.
    pub fn text_with_scope(&self) -> String {
        let diff_lines: Vec<&str> = self.text.lines().collect();
        let mut result = Vec::with_capacity(diff_lines.len());
        let mut hunk_idx = 0;

        for line in diff_lines {
            if line.starts_with("@@") {
                if let Some(hunk) = self.hunks.get(hunk_idx) {
                    if let Some(innermost) = hunk.ancestors.last() {
                        result.push(format!("{} {}", line, innermost.text.trim()));
                    } else {
                        result.push(line.to_string());
                    }
                    hunk_idx += 1;
                } else {
                    result.push(line.to_string());
                }
            } else {
                result.push(line.to_string());
            }
        }

        result.join("\n")
    }

    /// Returns the enriched hunks.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }
}

// ---------------------------------------------------------------------------
// Scope-expanded context helpers
// ---------------------------------------------------------------------------

/// Compute default 3-line context range for a change.
fn default_context_range(old_start: usize, old_end: usize, total_old: usize) -> ContextRange {
    let start = old_start.saturating_sub(DEFAULT_CONTEXT);
    let end = (old_end + DEFAULT_CONTEXT).min(total_old.saturating_sub(1));
    ContextRange { start, end }
}

/// Compute context ranges for each change operation.
///
/// For each change, determines whether to use scope-expanded context
/// (innermost scope ≤ MAX_SCOPE_LINES) or fall back to 3-line default.
fn compute_context_ranges(
    ops: &[similar::DiffOp],
    parsed: Option<&crate::syntax::ParsedFile>,
    source: &[u8],
    total_old: usize,
) -> Vec<ContextRange> {
    let mut ranges = Vec::new();

    for op in ops {
        let (old_start, old_end) = match op {
            similar::DiffOp::Equal { old_index, len, .. } => {
                // Skip equal ranges - no change here
                let _ = (old_index, len);
                continue;
            }
            similar::DiffOp::Delete { old_index, old_len, .. } => {
                (*old_index, old_index + old_len)
            }
            similar::DiffOp::Insert { old_index, .. } => {
                // Insert at old_index - use surrounding context
                (*old_index, *old_index)
            }
            similar::DiffOp::Replace { old_index, old_len, .. } => {
                (*old_index, old_index + old_len)
            }
        };

        // Try scope expansion if we have parsed tree
        if let Some(p) = parsed {
            let change_line = if old_start < total_old { old_start } else { total_old.saturating_sub(1) };
            let scopes = enclosing_scopes(
                p.tree.root_node(),
                source,
                change_line,
                p.scope_kinds,
            );

            if let Some(innermost) = scopes.last() {
                let scope_lines = innermost.end_line.saturating_sub(innermost.start_line) + 1;
                if scope_lines <= MAX_SCOPE_LINES {
                    // Use scope-expanded context (convert 1-indexed to 0-indexed)
                    let start = innermost.start_line.saturating_sub(1);
                    let end = innermost.end_line.saturating_sub(1);
                    ranges.push(ContextRange { start, end });
                    continue;
                }
            }
        }

        // Fall back to default 3-line context
        ranges.push(default_context_range(old_start, old_end.saturating_sub(1), total_old));
    }

    ranges
}

/// Merge overlapping or adjacent context ranges.
fn merge_ranges(mut ranges: Vec<ContextRange>) -> Vec<ContextRange> {
    if ranges.is_empty() {
        return ranges;
    }

    ranges.sort_by_key(|r| r.start);

    let mut merged = vec![ranges[0]];
    for range in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        // Merge if overlapping or adjacent (end + 1 >= start)
        if last.end + 1 >= range.start {
            last.end = last.end.max(range.end);
        } else {
            merged.push(*range);
        }
    }

    merged
}

/// Build hunks from merged context ranges.
///
/// Each merged range becomes one hunk. Lines are collected from ops
/// that fall within the range, and ancestors are computed from the
/// first change line in each hunk.
fn build_hunks_from_ranges(
    ops: &[similar::DiffOp],
    ranges: &[ContextRange],
    parsed: Option<&crate::syntax::ParsedFile>,
    source: &[u8],
    old_lines: &[&str],
    new_lines: &[&str],
) -> Vec<Hunk> {
    let mut hunks = Vec::new();

    for range in ranges {
        let mut lines = Vec::new();
        let mut first_change_old_line: Option<usize> = None;
        let mut new_start: Option<usize> = None;

        // Walk through ops and collect lines that fall within this range
        for op in ops {
            match op {
                similar::DiffOp::Equal { old_index, new_index, len } => {
                    for i in 0..*len {
                        let old_line = old_index + i;
                        if old_line >= range.start && old_line <= range.end {
                            if new_start.is_none() {
                                new_start = Some(new_index + i + 1);
                            }
                            lines.push(DiffLine {
                                kind: LineKind::Context,
                                content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
                            });
                        }
                    }
                }
                similar::DiffOp::Delete { old_index, old_len, new_index } => {
                    for i in 0..*old_len {
                        let old_line = old_index + i;
                        if old_line >= range.start && old_line <= range.end {
                            if new_start.is_none() {
                                new_start = Some(*new_index + 1);
                            }
                            if first_change_old_line.is_none() {
                                first_change_old_line = Some(old_line);
                            }
                            lines.push(DiffLine {
                                kind: LineKind::Removed,
                                content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
                            });
                        }
                    }
                }
                similar::DiffOp::Insert { old_index, new_index, new_len } => {
                    // Insert happens "at" old_index - include if range contains old_index
                    // or if range.start == old_index (insert at range boundary)
                    if *old_index >= range.start && *old_index <= range.end + 1 {
                        if new_start.is_none() {
                            new_start = Some(*new_index + 1);
                        }
                        if first_change_old_line.is_none() {
                            first_change_old_line = Some(*old_index);
                        }
                        for i in 0..*new_len {
                            lines.push(DiffLine {
                                kind: LineKind::Added,
                                content: new_lines.get(new_index + i).copied().unwrap_or("").to_string(),
                            });
                        }
                    }
                }
                similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    let mut added_in_range = false;
                    for i in 0..*old_len {
                        let old_line = old_index + i;
                        if old_line >= range.start && old_line <= range.end {
                            if new_start.is_none() {
                                new_start = Some(*new_index + 1);
                            }
                            if first_change_old_line.is_none() {
                                first_change_old_line = Some(old_line);
                            }
                            lines.push(DiffLine {
                                kind: LineKind::Removed,
                                content: old_lines.get(old_line).copied().unwrap_or("").to_string(),
                            });
                            added_in_range = true;
                        }
                    }
                    if added_in_range {
                        for i in 0..*new_len {
                            lines.push(DiffLine {
                                kind: LineKind::Added,
                                content: new_lines.get(new_index + i).copied().unwrap_or("").to_string(),
                            });
                        }
                    }
                }
            }
        }

        if lines.is_empty() {
            continue;
        }

        // Compute ancestors from first change line
        let ancestors = match (parsed, first_change_old_line) {
            (Some(p), Some(line)) => {
                enclosing_scopes(p.tree.root_node(), source, line, p.scope_kinds)
            }
            _ => Vec::new(),
        };

        hunks.push(Hunk {
            old_start: range.start + 1, // Convert to 1-indexed
            new_start: new_start.unwrap_or(1),
            lines,
            ancestors,
        });
    }

    hunks
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
    let point = Point::new(line, 0);
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
                .and_then(|name_node| name_node.utf8_text(source).ok())
                .unwrap_or("")
                .to_string();
            let text = source_line_raw(source, n.start_position().row)
                .unwrap_or_default();
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
        let context_count = hunks[0].lines.iter()
            .filter(|l| l.kind == LineKind::Context)
            .count();
        // 10 lines total, 1 changed = 9 context lines
        assert_eq!(context_count, 9, "should have full function as context");
    }

    #[test]
    fn expanded_large_scope_uses_default() {
        // A function > 50 lines should fall back to 3-line context
        let mut lines = vec!["fn big() {".to_string()];
        for i in 1..=55 {
            lines.push(format!("    let x{} = {};", i, i));
        }
        lines.push("}".to_string());
        let original = lines.join("\n") + "\n";
        let updated = original.replace("let x30 = 30", "let x30 = 3000");
        let diff = Diff::compute(&original, &updated, "test.rs");
        let hunks = diff.hunks();
        assert_eq!(hunks.len(), 1);
        // Should NOT start at line 1 (function start), should be close to change
        assert!(hunks[0].old_start > 1, "large scope should use default context");
        // Should have <= 6 context lines (3 before + 3 after)
        let context_count = hunks[0].lines.iter()
            .filter(|l| l.kind == LineKind::Context)
            .count();
        assert!(context_count <= 6, "large scope should use ~3-line context, got {}", context_count);
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
        assert!(total_lines <= 8, "top-level should use 3-line context, got {} lines", total_lines);
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
        assert!(total_lines <= 8, "unsupported lang should use 3-line context, got {} lines", total_lines);
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
        assert_eq!(hunks.len(), 1, "same function changes should merge to 1 hunk");
        // Should have 2 changed lines (a and d)
        let removed = hunks[0].lines.iter()
            .filter(|l| l.kind == LineKind::Removed)
            .count();
        let added = hunks[0].lines.iter()
            .filter(|l| l.kind == LineKind::Added)
            .count();
        assert_eq!(removed, 2, "should have 2 removed lines");
        assert_eq!(added, 2, "should have 2 added lines");
    }

    #[test]
    fn text_returns_standard_context() {
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
        // The @@ header should show limited context (not from line 1)
        assert!(text.contains("@@"));
        // Standard diff should NOT include line 1 (fn small())
        // because the change is on line 5 with 3-line context
        let lines: Vec<&str> = text.lines().collect();
        let has_fn_line = lines.iter().any(|l| l.contains("fn small()") && !l.starts_with("@@"));
        assert!(!has_fn_line, "standard text() should use 3-line context, not full scope");
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
        assert!(hunks[0].lines[0].content.contains("fn small()"), 
            "first context line should be function signature");
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
    fn diff_text_returns_plain_diff() {
        let original = "fn foo() {\n    1\n}\n";
        let updated = "fn foo() {\n    2\n}\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let text = diff.text();
        // Plain diff @@ header should end with @@ (no scope appended)
        let header = text.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(header.ends_with("@@"), "expected header to end with @@, got: {}", header);
    }

    #[test]
    fn diff_text_with_scope_injects_innermost_ancestor() {
        let original = "fn compute() {\n    let x = 1;\n}\n";
        let updated = "fn compute() {\n    let x = 2;\n}\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let with_scope = diff.text_with_scope();
        // Should have scope context appended to @@ line
        assert!(with_scope.contains("@@ -1,3 +1,3 @@ fn compute() {"));
    }

    #[test]
    fn diff_text_with_scope_nested_shows_innermost() {
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

        let with_scope = diff.text_with_scope();
        // Should show innermost (function), not impl
        assert!(with_scope.contains("fn compute(&self) -> i32 {"));
    }

    #[test]
    fn diff_text_with_scope_no_ancestors_unchanged() {
        let original = "let x = 1;\nlet y = 2;\n";
        let updated = "let x = 1;\nlet y = 3;\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let plain = diff.text();
        let with_scope = diff.text_with_scope();
        // Top-level code has no ancestors, so @@ line should be unchanged
        assert!(with_scope.contains("@@ -1,2 +1,2 @@"));
        // Both should have the same @@ line (no scope appended)
        let plain_header = plain.lines().find(|l| l.starts_with("@@")).unwrap();
        let scope_header = with_scope.lines().find(|l| l.starts_with("@@")).unwrap();
        assert_eq!(plain_header, scope_header);
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
}
