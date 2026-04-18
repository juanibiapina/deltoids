//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! injected into unified diff `@@` hunk headers so the TUI can display which
//! function a change belongs to.

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Point};

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
}
