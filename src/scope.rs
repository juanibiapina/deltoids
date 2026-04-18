//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! injected into unified diff `@@` hunk headers so the TUI can display which
//! function a change belongs to.

use serde::{Deserialize, Serialize};
use similar::{DiffOp, DiffTag, TextDiff};
use tree_sitter::{Node, Point};

const MAX_SCOPE_LINES: usize = 50;
const DEFAULT_CONTEXT: usize = 3;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNode {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkScopes {
    pub hunk_old_start: usize,
    pub hunk_new_start: usize,
    pub ancestors: Vec<ScopeNode>,
}

// ---------------------------------------------------------------------------
// Core algorithm
// ---------------------------------------------------------------------------

/// Find the first source line of the innermost enclosing scope node.
/// Returns the line trimmed of leading whitespace, like delta/git.
fn enclosing_scope(
    root: Node,
    source: &[u8],
    line: usize,
    scope_kinds: &[&str],
) -> Option<String> {
    let point = Point::new(line, 0);
    let node = root.descendant_for_point_range(point, point)?;

    let mut current = Some(node);
    while let Some(n) = current {
        if scope_kinds.contains(&n.kind()) {
            let start_line = n.start_position().row;
            return source_first_line(source, start_line);
        }
        current = n.parent();
    }
    None
}

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

/// Return the 0-indexed source line, trimmed of leading whitespace.
fn source_first_line(source: &[u8], line: usize) -> Option<String> {
    let text = std::str::from_utf8(source).ok()?;
    text.lines().nth(line).map(|l| l.trim_start().to_string())
}

/// Parse the old-file start line from a unified diff hunk header.
/// Input: `@@ -74,15 +75,14 @@` -> Some(74)
fn parse_hunk_old_start(line: &str) -> Option<usize> {
    let after = line.strip_prefix("@@ -")?;
    let end = after.find([',', ' '])?;
    after[..end].parse().ok()
}

/// Parse both old-file and new-file start lines from a unified diff hunk header.
/// Input: `@@ -74,15 +75,14 @@` -> Some((74, 75))
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let old_start = parse_hunk_old_start(line)?;
    let after_plus = line.find('+').map(|i| &line[i + 1..])?;
    let end = after_plus.find([',', ' '])?;
    let new_start = after_plus[..end].parse().ok()?;
    Some((old_start, new_start))
}

/// Compute structural scope ancestors for each hunk in a diff.
///
/// The `diff` should be the raw unified diff (before scope injection).
/// The `original` is the original file content.
pub fn compute_hunk_scopes(diff: &str, original: &str, path: &str) -> Vec<HunkScopes> {
    let parsed = match crate::syntax::parse_file(path, original) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let root = parsed.tree.root_node();
    let source = original.as_bytes();
    let diff_lines: Vec<&str> = diff.lines().collect();
    let mut result = Vec::new();

    for (i, &line) in diff_lines.iter().enumerate() {
        if !line.starts_with("@@") {
            continue;
        }

        let Some((old_start, new_start)) = parse_hunk_header(line) else {
            continue;
        };

        // Walk forward from the @@ line to find the first changed line.
        let mut offset = 0usize;
        let mut change_line = None;
        for l in &diff_lines[(i + 1)..] {
            if l.starts_with("@@") || l.starts_with("---") || l.starts_with("+++") {
                break;
            }
            if l.starts_with('-') || l.starts_with('+') {
                change_line = Some(old_start + offset);
                break;
            }
            if l.starts_with(' ') {
                offset += 1;
            }
        }

        let ancestors = match change_line {
            Some(cl) => {
                let ts_line = cl.saturating_sub(1);
                enclosing_scopes(root, source, ts_line, parsed.scope_kinds)
            }
            None => Vec::new(),
        };

        result.push(HunkScopes {
            hunk_old_start: old_start,
            hunk_new_start: new_start,
            ancestors,
        });
    }

    result
}

// ---------------------------------------------------------------------------
// Scope-expanded diff
// ---------------------------------------------------------------------------

/// Generate a unified diff with scope-expanded context.
///
/// For each change, if the innermost enclosing scope is ≤ 50 lines,
/// the context is expanded to cover the full scope. Otherwise, 3 lines
/// of context are used (the standard default).
///
/// Falls back to standard 3-line context for unsupported languages.
pub fn scope_expanded_diff(original: &str, updated: &str, path: &str) -> String {
    let text_diff = TextDiff::from_lines(original, updated);
    let ops = text_diff.ops().to_vec();

    // No changes -> empty string (matching similar's behavior)
    if ops.iter().all(|op| op.tag() == DiffTag::Equal) {
        return String::new();
    }

    // Parse for scope info; fallback to standard diff if unsupported
    let parsed = match crate::syntax::parse_file(path, original) {
        Some(p) => p,
        None => return crate::raw_unified_diff(original, updated),
    };

    let root = parsed.tree.root_node();
    let source_bytes = original.as_bytes();
    let total_old = text_diff.old_len();

    let merged = compute_merged_context_ranges(
        &ops,
        root,
        source_bytes,
        parsed.scope_kinds,
        total_old,
    );

    if merged.is_empty() {
        return String::new();
    }

    // Build hunks
    use std::fmt::Write;
    let mut output = String::new();
    writeln!(output, "--- original").unwrap();
    writeln!(output, "+++ modified").unwrap();

    for &(ctx_start, ctx_end) in &merged {
        write_hunk(&text_diff, &ops, ctx_start, ctx_end, &mut output);
    }

    output
}

fn compute_merged_context_ranges(
    ops: &[DiffOp],
    root: Node,
    source_bytes: &[u8],
    scope_kinds: &[&str],
    total_old: usize,
) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    for op in ops {
        if op.tag() == DiffTag::Equal {
            continue;
        }

        let old_range = op.old_range();
        let scope_line = if old_range.is_empty() {
            old_range.start.saturating_sub(1)
        } else {
            old_range.start
        };

        let scopes = enclosing_scopes(root, source_bytes, scope_line, scope_kinds);
        let innermost = scopes.last();

        let (start, end) = match innermost {
            Some(s) if (s.end_line - s.start_line + 1) <= MAX_SCOPE_LINES => {
                // Convert 1-indexed to 0-indexed
                (s.start_line - 1, s.end_line - 1)
            }
            _ => default_context_range(&old_range, total_old),
        };

        ranges.push((start, end));
    }

    // Merge overlapping/adjacent ranges
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    merged
}

fn default_context_range(
    old_range: &std::ops::Range<usize>,
    total_old: usize,
) -> (usize, usize) {
    let start = old_range.start.saturating_sub(DEFAULT_CONTEXT);
    let change_end = if old_range.is_empty() {
        old_range.start
    } else {
        old_range.end
    };
    let end = if total_old == 0 {
        0
    } else {
        (change_end + DEFAULT_CONTEXT - 1).min(total_old - 1)
    };
    (start, end)
}

fn write_hunk(
    text_diff: &TextDiff<'_, '_, str>,
    ops: &[DiffOp],
    ctx_start: usize,
    ctx_end: usize,
    out: &mut String,
) {
    use std::fmt::Write;

    let hunk_old_start = ctx_start;
    let mut hunk_new_start: Option<usize> = None;
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    let mut body = String::new();

    for op in ops {
        let old_range = op.old_range();
        let new_range = op.new_range();

        match op.tag() {
            DiffTag::Equal => {
                let vis_start = old_range.start.max(ctx_start);
                let vis_end = old_range.end.min(ctx_end + 1);
                if vis_start >= vis_end {
                    continue;
                }

                let offset = vis_start - old_range.start;
                if hunk_new_start.is_none() {
                    hunk_new_start = Some(new_range.start + offset);
                }

                for i in vis_start..vis_end {
                    let slice = text_diff.old_slice(i).unwrap();
                    write!(body, " {slice}").unwrap();
                    if !slice.ends_with('\n') {
                        body.push('\n');
                        body.push_str("\\ No newline at end of file\n");
                    }
                    old_count += 1;
                    new_count += 1;
                }
            }
            DiffTag::Delete => {
                if old_range.end <= ctx_start || old_range.start > ctx_end {
                    continue;
                }

                if hunk_new_start.is_none() {
                    hunk_new_start = Some(new_range.start);
                }

                for i in old_range.clone() {
                    let slice = text_diff.old_slice(i).unwrap();
                    write!(body, "-{slice}").unwrap();
                    if !slice.ends_with('\n') {
                        body.push('\n');
                        body.push_str("\\ No newline at end of file\n");
                    }
                    old_count += 1;
                }
            }
            DiffTag::Insert => {
                if old_range.start < ctx_start || old_range.start > ctx_end + 1 {
                    continue;
                }

                if hunk_new_start.is_none() {
                    hunk_new_start = Some(new_range.start);
                }

                for i in new_range.clone() {
                    let slice = text_diff.new_slice(i).unwrap();
                    write!(body, "+{slice}").unwrap();
                    if !slice.ends_with('\n') {
                        body.push('\n');
                        body.push_str("\\ No newline at end of file\n");
                    }
                    new_count += 1;
                }
            }
            DiffTag::Replace => {
                if old_range.end <= ctx_start || old_range.start > ctx_end {
                    continue;
                }

                if hunk_new_start.is_none() {
                    hunk_new_start = Some(new_range.start);
                }

                for i in old_range.clone() {
                    let slice = text_diff.old_slice(i).unwrap();
                    write!(body, "-{slice}").unwrap();
                    if !slice.ends_with('\n') {
                        body.push('\n');
                        body.push_str("\\ No newline at end of file\n");
                    }
                    old_count += 1;
                }
                for i in new_range.clone() {
                    let slice = text_diff.new_slice(i).unwrap();
                    write!(body, "+{slice}").unwrap();
                    if !slice.ends_with('\n') {
                        body.push('\n');
                        body.push_str("\\ No newline at end of file\n");
                    }
                    new_count += 1;
                }
            }
        }
    }

    if body.is_empty() {
        return;
    }

    let hunk_new_start = hunk_new_start.unwrap_or(0);
    let old_hdr = format_hunk_range(hunk_old_start, old_count);
    let new_hdr = format_hunk_range(hunk_new_start, new_count);
    writeln!(out, "@@ -{old_hdr} +{new_hdr} @@").unwrap();
    out.push_str(&body);
}

fn format_hunk_range(start: usize, count: usize) -> String {
    let display_start = start + 1;
    match count {
        0 => format!("{},{}", display_start.saturating_sub(1), 0),
        1 => format!("{display_start}"),
        _ => format!("{display_start},{count}"),
    }
}

/// Inject scope context into `@@` hunk headers of a unified diff.
///
/// For each hunk, finds the first changed line and looks up its enclosing
/// scope in the original file. The scope label is appended after the
/// closing `@@`:
///
///   `@@ -13,7 +13,7 @@ fn compute(&self) -> i32 {`
pub fn inject_scope_context(diff: &str, original: &str, path: &str) -> String {
    let parsed = match crate::syntax::parse_file(path, original) {
        Some(p) => p,
        None => return diff.to_string(),
    };
    let root = parsed.tree.root_node();
    let source = original.as_bytes();

    let diff_lines: Vec<&str> = diff.lines().collect();
    let mut result = Vec::with_capacity(diff_lines.len());

    for (i, &line) in diff_lines.iter().enumerate() {
        if !line.starts_with("@@") {
            result.push(line.to_string());
            continue;
        }

        let scope = parse_hunk_old_start(line).and_then(|start| {
            // Walk forward from the @@ line to find the first changed line.
            // Count context lines to determine the old-file line of the change.
            let mut offset = 0usize;
            for l in &diff_lines[(i + 1)..] {
                if l.starts_with("@@") || l.starts_with("---") || l.starts_with("+++") {
                    break;
                }
                if l.starts_with('-') || l.starts_with('+') {
                    let change_line = start + offset;
                    // Convert 1-indexed diff line to 0-indexed tree-sitter line.
                    let ts_line = change_line.saturating_sub(1);
                    return enclosing_scope(root, source, ts_line, parsed.scope_kinds);
                }
                if l.starts_with(' ') {
                    offset += 1;
                }
            }
            None
        });

        match scope {
            Some(s) => result.push(format!("{line} {s}")),
            None => result.push(line.to_string()),
        }
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use similar::TextDiff;

    /// Generate a plain unified diff without scope injection.
    fn raw_diff(original: &str, updated: &str) -> String {
        let text_diff = TextDiff::from_lines(original, updated);
        let mut diff = text_diff.unified_diff();
        diff.context_radius(3).header("original", "modified");
        diff.to_string()
    }

    #[test]
    fn parses_hunk_old_start() {
        assert_eq!(parse_hunk_old_start("@@ -74,15 +75,14 @@"), Some(74));
        assert_eq!(parse_hunk_old_start("@@ -1 +1,3 @@"), Some(1));
        assert_eq!(parse_hunk_old_start("not a hunk"), None);
    }

    #[test]
    fn injects_rust_scope_with_full_signature() {
        let original = "\
fn foo() {
    let x = 1;
    let y = 2;
}

fn bar(a: i32, b: i32) -> i32 {
    let a = 1;
    let b = 2;
    let c = 3;
    a + b + c
}
";
        let updated = original.replace("let b = 2", "let b = 99");
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "test.rs");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(
            hunk_line.contains("fn bar(a: i32, b: i32) -> i32 {"),
            "expected full signature, got: {hunk_line}"
        );
    }

    #[test]
    fn injects_innermost_scope() {
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
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "src/lib.rs");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        // Should show the innermost scope's source line, not a synthetic chain.
        assert!(
            hunk_line.contains("fn compute(&self) -> i32 {"),
            "expected innermost scope source line, got: {hunk_line}"
        );
    }

    #[test]
    fn injects_python_scope() {
        let original = "\
class Calc:
    def add(self, a, b):
        return a + b

    def sub(self, a, b):
        return a - b
";
        let updated = original.replace("a - b", "a - b - 1");
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "calc.py");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(
            hunk_line.contains("def sub(self, a, b):"),
            "expected Python source line, got: {hunk_line}"
        );
    }

    #[test]
    fn injects_javascript_scope() {
        let original = "\
class Foo {
    getValue() {
        return 1;
    }
}
";
        let updated = original.replace("return 1", "return 2");
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "foo.js");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(
            hunk_line.contains("getValue() {"),
            "expected JS source line, got: {hunk_line}"
        );
    }

    #[test]
    fn injects_go_scope() {
        let original = "\
package main

func hello() {
\tprintln(\"hi\")
}
";
        let updated = original.replace("\"hi\"", "\"hello\"");
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "main.go");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(
            hunk_line.contains("func hello() {"),
            "expected Go source line, got: {hunk_line}"
        );
    }

    #[test]
    fn unknown_extension_passes_through() {
        let diff = "@@ -1,3 +1,3 @@\n context\n-old\n+new";
        let result = inject_scope_context(diff, "some content\n", "data.xyz");
        assert_eq!(result, diff);
    }

    #[test]
    fn no_scope_at_top_level() {
        let original = "let x = 1;\nlet y = 2;\n";
        let updated = "let x = 1;\nlet y = 3;\n";
        let diff = raw_diff(original, updated);
        let enriched = inject_scope_context(&diff, original, "top.rs");
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        // No scope should be appended; the line should end with @@
        assert!(
            hunk_line.ends_with("@@"),
            "expected no scope at top level, got: {hunk_line}"
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
    // parse_hunk_header tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_hunk_header_extracts_both_starts() {
        assert_eq!(parse_hunk_header("@@ -74,15 +75,14 @@"), Some((74, 75)));
        assert_eq!(parse_hunk_header("@@ -1 +1,3 @@"), Some((1, 1)));
        assert_eq!(parse_hunk_header("not a hunk"), None);
    }

    // -----------------------------------------------------------------------
    // compute_hunk_scopes tests
    // -----------------------------------------------------------------------

    #[test]
    fn compute_hunk_scopes_returns_one_per_hunk() {
        let original = "\
impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        let updated = original.replace("x + 1", "x + 2");
        let diff = raw_diff(original, &updated);
        let scopes = compute_hunk_scopes(&diff, original, "test.rs");
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].ancestors.len(), 2);
        assert_eq!(scopes[0].ancestors[0].kind, "impl_item");
        assert_eq!(scopes[0].ancestors[1].kind, "function_item");
    }

    #[test]
    fn compute_hunk_scopes_returns_empty_for_unsupported_language() {
        let diff = "@@ -1,3 +1,3 @@\n context\n-old\n+new";
        let scopes = compute_hunk_scopes(diff, "some content\n", "data.xyz");
        assert!(scopes.is_empty());
    }

    #[test]
    fn compute_hunk_scopes_multi_hunk_diff() {
        // Pad with enough lines between functions to force separate hunks
        // (context_radius=3, so we need > 6 lines gap).
        let original = "\
fn first() {
    let a = 1;
}







fn second() {
    let b = 2;
}
";
        let updated = original.replace("let a = 1", "let a = 10").replace("let b = 2", "let b = 20");
        let diff = raw_diff(original, &updated);
        let scopes = compute_hunk_scopes(&diff, original, "test.rs");
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].ancestors[0].name, "first");
        assert_eq!(scopes[1].ancestors[0].name, "second");
    }

    #[test]
    fn compute_hunk_scopes_stores_hunk_starts() {
        let original = "\
impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
        let updated = original.replace("x + 1", "x + 2");
        let diff = raw_diff(original, &updated);
        let scopes = compute_hunk_scopes(&diff, original, "test.rs");
        assert_eq!(scopes.len(), 1);
        // Both old and new start should be populated from the @@ header
        assert!(scopes[0].hunk_old_start > 0);
        assert!(scopes[0].hunk_new_start > 0);
    }

    // -----------------------------------------------------------------------
    // scope_expanded_diff tests
    // -----------------------------------------------------------------------

    #[test]
    fn expanded_diff_covers_full_function() {
        // A 10-line function: change inside should show the full function.
        let original = "\
fn setup() {}

fn compute(x: i32) -> i32 {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = x + a + b + c + d;
    e
}

fn teardown() {}
";
        let updated = original.replace("let c = 3", "let c = 99");
        let diff = scope_expanded_diff(original, &updated, "test.rs");

        // The full function should be visible as context
        assert!(diff.contains(" fn compute(x: i32) -> i32 {"), "should show fn header");
        assert!(diff.contains(" }\n"), "should show closing brace");
        assert!(diff.contains("-    let c = 3;"), "should show removed line");
        assert!(diff.contains("+    let c = 99;"), "should show added line");
    }

    #[test]
    fn expanded_diff_covers_full_struct() {
        let original = "\
fn unrelated() {}

struct Config {
    name: String,
    value: i32,
    enabled: bool,
}

fn also_unrelated() {}
";
        let updated = original.replace("value: i32", "value: u64");
        let diff = scope_expanded_diff(original, &updated, "test.rs");

        assert!(diff.contains(" struct Config {"), "should show struct header");
        assert!(diff.contains(" }\n"), "should show closing brace");
        assert!(!diff.contains("unrelated"), "should not show unrelated functions");
    }

    #[test]
    fn expanded_diff_top_level_uses_3_line_context() {
        // Top-level change (no enclosing scope) should use 3-line context.
        let original = "\
let a = 1;
let b = 2;
let c = 3;
let d = 4;
let e = 5;
let f = 6;
let g = 7;
let h = 8;
";
        let updated = original.replace("let e = 5", "let e = 99");
        let diff = scope_expanded_diff(original, &updated, "test.rs");

        // 3 lines of context before and after the change
        assert!(diff.contains(" let b = 2;\n"), "should show 3rd line before change");
        assert!(diff.contains(" let h = 8;\n"), "should show 3rd line after change");
        assert!(!diff.contains("let a = 1"), "should not show 4th line before change");
    }

    #[test]
    fn expanded_diff_large_scope_falls_back_to_3_lines() {
        // Build a function with > 50 lines.
        let mut lines = vec!["fn big_function() {".to_string()];
        for i in 0..55 {
            lines.push(format!("    let x{i} = {i};"));
        }
        lines.push("}".to_string());
        lines.push(String::new());
        let original = lines.join("\n");

        // Change a line in the middle (line ~30)
        let updated = original.replace("let x30 = 30", "let x30 = 999");
        let diff = scope_expanded_diff(&original, &updated, "test.rs");

        // Should NOT show the full function (too big)
        assert!(!diff.contains("fn big_function"), "should not show full scope > 50 lines");
        // Should show 3-line context
        assert!(diff.contains("let x27"), "should show 3rd line before change");
        assert!(diff.contains("let x33"), "should show 3rd line after change");
    }

    #[test]
    fn expanded_diff_two_functions_separate_hunks() {
        // Two functions with enough gap between them to force separate hunks.
        let original = "\
fn first() {
    let a = 1;
    let b = 2;
}







fn second() {
    let c = 3;
    let d = 4;
}
";
        let updated = original
            .replace("let a = 1", "let a = 10")
            .replace("let c = 3", "let c = 30");
        let diff = scope_expanded_diff(original, &updated, "test.rs");

        // Should have two separate hunks
        let hunk_count = diff.lines().filter(|l| l.starts_with("@@")).count();
        assert_eq!(hunk_count, 2, "expected 2 hunks, got diff:\n{diff}");

        // Each hunk should cover its function
        assert!(diff.contains(" fn first()"), "should show first function header");
        assert!(diff.contains(" fn second()"), "should show second function header");
    }

    #[test]
    fn expanded_diff_two_changes_same_function_one_hunk() {
        let original = "\
fn compute() -> i32 {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    a + b + c + d
}
";
        let updated = original
            .replace("let a = 1", "let a = 10")
            .replace("let d = 4", "let d = 40");
        let diff = scope_expanded_diff(original, &updated, "test.rs");

        // Both changes are in the same function, so one hunk
        let hunk_count = diff.lines().filter(|l| l.starts_with("@@")).count();
        assert_eq!(hunk_count, 1, "expected 1 hunk, got diff:\n{diff}");
    }

    #[test]
    fn expanded_diff_unsupported_language_uses_3_line_context() {
        let original = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n";
        let updated = original.replace("line5", "LINE5");
        let diff = scope_expanded_diff(original, &updated, "data.xyz");

        // Unsupported language falls back to standard 3-line context
        assert!(diff.contains("--- original"));
        assert!(diff.contains("+++ modified"));
        assert!(diff.contains("-line5"));
        assert!(diff.contains("+LINE5"));
        // 3-line context
        assert!(diff.contains(" line2\n"), "should show 3rd line before change");
        assert!(diff.contains(" line8\n"), "should show 3rd line after change");
    }
}
