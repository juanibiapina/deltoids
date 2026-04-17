//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! injected into unified diff `@@` hunk headers so the TUI can display which
//! function a change belongs to.

use std::path::Path;

use tree_sitter::{Node, Parser, Point};
use tree_sitter_language::LanguageFn;

/// Scope node kinds per language. Tree-sitter finds the enclosing scope
/// node and we use its first source line as context (like delta/git).
struct LangConfig {
    language: LanguageFn,
    scope_kinds: &'static [&'static str],
}

// ---------------------------------------------------------------------------
// Language registry
// ---------------------------------------------------------------------------

/// Detect language from file extension and return its config.
fn lang_config(path: &str) -> Option<LangConfig> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(LangConfig {
            language: tree_sitter_rust::LANGUAGE,
            scope_kinds: &[
                "function_item",
                "impl_item",
                "struct_item",
                "enum_item",
                "trait_item",
                "mod_item",
            ],
        }),
        "py" | "pyi" => Some(LangConfig {
            language: tree_sitter_python::LANGUAGE,
            scope_kinds: &["function_definition", "class_definition"],
        }),
        "js" | "mjs" | "cjs" | "jsx" => Some(LangConfig {
            language: tree_sitter_javascript::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
        }),
        "ts" | "mts" | "cts" => Some(LangConfig {
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
        }),
        "tsx" => Some(LangConfig {
            language: tree_sitter_typescript::LANGUAGE_TSX,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
        }),
        "go" => Some(LangConfig {
            language: tree_sitter_go::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        }),
        "rb" | "rake" | "gemspec" => Some(LangConfig {
            language: tree_sitter_ruby::LANGUAGE,
            scope_kinds: &["method", "singleton_method", "class", "module"],
        }),
        "java" => Some(LangConfig {
            language: tree_sitter_java::LANGUAGE,
            scope_kinds: &[
                "class_declaration",
                "interface_declaration",
                "method_declaration",
                "constructor_declaration",
            ],
        }),
        "c" | "h" => Some(LangConfig {
            language: tree_sitter_c::LANGUAGE,
            scope_kinds: &["function_definition", "struct_specifier"],
        }),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some(LangConfig {
            language: tree_sitter_cpp::LANGUAGE,
            scope_kinds: &[
                "function_definition",
                "class_specifier",
                "namespace_definition",
            ],
        }),
        "sh" | "bash" | "zsh" => Some(LangConfig {
            language: tree_sitter_bash::LANGUAGE,
            scope_kinds: &["function_definition"],
        }),
        "lua" => Some(LangConfig {
            language: tree_sitter_lua::LANGUAGE,
            scope_kinds: &["function_declaration"],
        }),
        "css" | "scss" => Some(LangConfig {
            language: tree_sitter_css::LANGUAGE,
            scope_kinds: &["rule_set", "media_statement"],
        }),
        "tf" | "hcl" => Some(LangConfig {
            language: tree_sitter_hcl::LANGUAGE,
            scope_kinds: &["block"],
        }),
        _ => None,
    }
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
    config: &LangConfig,
) -> Option<String> {
    let point = Point::new(line, 0);
    let node = root.descendant_for_point_range(point, point)?;

    let mut current = Some(node);
    while let Some(n) = current {
        if config.scope_kinds.contains(&n.kind()) {
            let start_line = n.start_position().row;
            return source_first_line(source, start_line);
        }
        current = n.parent();
    }
    None
}

/// Return the 0-indexed source line, trimmed of leading whitespace.
fn source_first_line(source: &[u8], line: usize) -> Option<String> {
    let text = std::str::from_utf8(source).ok()?;
    text.lines()
        .nth(line)
        .map(|l| l.trim_start().to_string())
}

/// Parse the old-file start line from a unified diff hunk header.
/// Input: `@@ -74,15 +75,14 @@` -> Some(74)
fn parse_hunk_old_start(line: &str) -> Option<usize> {
    let after = line.strip_prefix("@@ -")?;
    let end = after.find([',', ' '])?;
    after[..end].parse().ok()
}

/// Inject scope context into `@@` hunk headers of a unified diff.
///
/// For each hunk, finds the first changed line and looks up its enclosing
/// scope in the original file. The scope label is appended after the
/// closing `@@`:
///
///   `@@ -13,7 +13,7 @@ fn compute(&self) -> i32 {`
pub fn inject_scope_context(diff: &str, original: &str, path: &str) -> String {
    let config = match lang_config(path) {
        Some(c) => c,
        None => return diff.to_string(),
    };

    let mut parser = Parser::new();
    if parser.set_language(&config.language.into()).is_err() {
        return diff.to_string();
    }
    let tree = match parser.parse(original, None) {
        Some(t) => t,
        None => return diff.to_string(),
    };
    let root = tree.root_node();
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
                    return enclosing_scope(root, source, ts_line, &config);
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
}
