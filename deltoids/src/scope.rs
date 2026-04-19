//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! used by the TUI to display which function a change belongs to.

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Point};

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

/// A unified diff enriched with tree-sitter scope information.
///
/// Use `Diff::compute()` to create a diff from original and updated content.
/// The diff provides both raw unified diff text and structured hunks with
/// ancestor scope chains.
#[derive(Debug, Clone)]
pub struct Diff {
    hunks: Vec<Hunk>,
    raw_text: String,
}

impl Diff {
    /// Compute a diff between original and updated content.
    ///
    /// Parses the file using tree-sitter (if the language is supported) to
    /// populate each hunk's ancestor scope chain.
    pub fn compute(original: &str, updated: &str, path: &str) -> Self {
        use similar::TextDiff;

        let text_diff = TextDiff::from_lines(original, updated);
        let mut diff = text_diff.unified_diff();
        diff.context_radius(3).header("original", "modified");
        let raw_text = diff.to_string();

        let hunks = enrich_diff(&raw_text, original, path);

        Diff { hunks, raw_text }
    }

    /// Returns the raw unified diff text.
    pub fn to_unified(&self) -> &str {
        &self.raw_text
    }

    /// Returns unified diff text with scope context injected into @@ headers.
    ///
    /// Each hunk header is appended with the innermost ancestor's source line
    /// (trimmed), making it easier to see which function/struct a change belongs to.
    pub fn to_unified_with_scope(&self) -> String {
        let diff_lines: Vec<&str> = self.raw_text.lines().collect();
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

/// Parse a unified diff and enrich each hunk with scope information.
///
/// Takes a raw unified diff and the original (old) file content.
/// Returns one `Hunk` per `@@` header, with lines parsed and ancestors populated.
pub fn enrich_diff(diff: &str, old_content: &str, path: &str) -> Vec<Hunk> {
    let parsed = crate::syntax::parse_file(path, old_content);
    let mut hunks = Vec::new();
    let lines: Vec<&str> = diff.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@") {
            let (old_start, new_start) = parse_hunk_header(line).unwrap_or((1, 1));
            let hunk_start_idx = i;
            let mut diff_lines = Vec::new();

            i += 1;
            while i < lines.len() && !lines[i].starts_with("@@") {
                let l = lines[i];
                if l.starts_with('+') && !l.starts_with("+++") {
                    diff_lines.push(DiffLine {
                        kind: LineKind::Added,
                        content: l[1..].to_string(),
                    });
                } else if l.starts_with('-') && !l.starts_with("---") {
                    diff_lines.push(DiffLine {
                        kind: LineKind::Removed,
                        content: l[1..].to_string(),
                    });
                } else if l.starts_with(' ') {
                    diff_lines.push(DiffLine {
                        kind: LineKind::Context,
                        content: l[1..].to_string(),
                    });
                }
                i += 1;
            }

            // Compute ancestors from first changed line
            let ancestors = match &parsed {
                Some(p) => {
                    let change_line = find_first_change_line(&lines, hunk_start_idx, old_start);
                    match change_line {
                        Some(cl) => {
                            let ts_line = cl.saturating_sub(1);
                            enclosing_scopes(
                                p.tree.root_node(),
                                old_content.as_bytes(),
                                ts_line,
                                p.scope_kinds,
                            )
                        }
                        None => Vec::new(),
                    }
                }
                None => Vec::new(),
            };

            hunks.push(Hunk {
                old_start,
                new_start,
                lines: diff_lines,
                ancestors,
            });
        } else {
            i += 1;
        }
    }

    hunks
}

/// Find the line number of the first changed line in a hunk.
fn find_first_change_line(lines: &[&str], hunk_start: usize, old_start: usize) -> Option<usize> {
    let mut offset = 0;
    for l in &lines[(hunk_start + 1)..] {
        if l.starts_with("@@") || l.starts_with("---") || l.starts_with("+++") {
            break;
        }
        if l.starts_with('-') || l.starts_with('+') {
            return Some(old_start + offset);
        }
        if l.starts_with(' ') {
            offset += 1;
        }
    }
    None
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

    // -----------------------------------------------------------------------
    // enrich_diff tests
    // -----------------------------------------------------------------------

    #[test]
    fn enrich_diff_empty_returns_empty() {
        let hunks = enrich_diff("", "", "test.rs");
        assert!(hunks.is_empty());
    }

    #[test]
    fn enrich_diff_single_added_line() {
        let diff = "\
--- original
+++ modified
@@ -1 +1,2 @@
 line1
+line2
";
        let hunks = enrich_diff(diff, "line1\n", "test.txt");
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
    fn enrich_diff_multiple_hunks() {
        let diff = "\
--- original
+++ modified
@@ -1,3 +1,3 @@
 line1
-line2
+LINE2
 line3
@@ -10,3 +10,3 @@
 line10
-line11
+LINE11
 line12
";
        let hunks = enrich_diff(diff, "", "test.txt");
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[1].old_start, 10);
    }

    #[test]
    fn enrich_diff_populates_ancestors_for_rust() {
        let original = "\
fn compute() {
    let x = 1;
    let y = 2;
}
";
        let updated = original.replace("let x = 1", "let x = 10");
        let diff = raw_diff(original, &updated);
        let hunks = enrich_diff(&diff, original, "test.rs");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.len(), 1);
        assert_eq!(hunks[0].ancestors[0].kind, "function_item");
        assert_eq!(hunks[0].ancestors[0].name, "compute");
    }

    #[test]
    fn enrich_diff_nested_scope_impl_and_function() {
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
        let hunks = enrich_diff(&diff, original, "test.rs");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ancestors.len(), 2);
        assert_eq!(hunks[0].ancestors[0].kind, "impl_item");
        assert_eq!(hunks[0].ancestors[0].name, "Foo");
        assert_eq!(hunks[0].ancestors[1].kind, "function_item");
        assert_eq!(hunks[0].ancestors[1].name, "compute");
    }

    #[test]
    fn enrich_diff_unsupported_language_empty_ancestors() {
        let diff = "\
--- original
+++ modified
@@ -1,3 +1,3 @@
 line1
-line2
+LINE2
 line3
";
        let hunks = enrich_diff(diff, "line1\nline2\nline3\n", "data.xyz");
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].ancestors.is_empty());
    }

    #[test]
    fn enrich_diff_top_level_code_empty_ancestors() {
        let original = "let x = 1;\nlet y = 2;\n";
        let updated = "let x = 1;\nlet y = 3;\n";
        let diff = raw_diff(original, updated);
        let hunks = enrich_diff(&diff, original, "test.rs");
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].ancestors.is_empty());
    }

    #[test]
    fn parses_hunk_old_start() {
        assert_eq!(parse_hunk_old_start("@@ -74,15 +75,14 @@"), Some(74));
        assert_eq!(parse_hunk_old_start("@@ -1 +1,3 @@"), Some(1));
        assert_eq!(parse_hunk_old_start("not a hunk"), None);
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
    // Diff tests
    // -----------------------------------------------------------------------

    #[test]
    fn diff_compute_produces_hunks_and_raw_text() {
        let original = "line1\nline2\n";
        let updated = "line1\nLINE2\n";
        let diff = Diff::compute(original, updated, "test.txt");

        assert_eq!(diff.hunks().len(), 1);
        assert!(diff.to_unified().contains("-line2"));
        assert!(diff.to_unified().contains("+LINE2"));
    }

    #[test]
    fn diff_to_unified_returns_plain_diff() {
        let original = "fn foo() {\n    1\n}\n";
        let updated = "fn foo() {\n    2\n}\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let unified = diff.to_unified();
        // Plain diff @@ header should end with @@ (no scope appended)
        let header = unified.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(header.ends_with("@@"), "expected header to end with @@, got: {}", header);
    }

    #[test]
    fn diff_to_unified_with_scope_injects_innermost_ancestor() {
        let original = "fn compute() {\n    let x = 1;\n}\n";
        let updated = "fn compute() {\n    let x = 2;\n}\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let with_scope = diff.to_unified_with_scope();
        // Should have scope context appended to @@ line
        assert!(with_scope.contains("@@ -1,3 +1,3 @@ fn compute() {"));
    }

    #[test]
    fn diff_to_unified_with_scope_nested_shows_innermost() {
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

        let with_scope = diff.to_unified_with_scope();
        // Should show innermost (function), not impl
        assert!(with_scope.contains("fn compute(&self) -> i32 {"));
    }

    #[test]
    fn diff_to_unified_with_scope_no_ancestors_unchanged() {
        let original = "let x = 1;\nlet y = 2;\n";
        let updated = "let x = 1;\nlet y = 3;\n";
        let diff = Diff::compute(original, updated, "test.rs");

        let plain = diff.to_unified();
        let with_scope = diff.to_unified_with_scope();
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
