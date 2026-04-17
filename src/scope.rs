//! Tree-sitter based scope context for diff hunk headers.
//!
//! Given a file path and source text, this module can determine the enclosing
//! scope (function, class, module, etc.) for any line number. This context is
//! injected into unified diff `@@` hunk headers so the TUI can display which
//! function a change belongs to.

use std::path::Path;

use tree_sitter::{Node, Parser, Point};
use tree_sitter_language::LanguageFn;

/// Scope node kinds and how to extract a label from each, per language.
struct LangConfig {
    language: LanguageFn,
    scope_kinds: &'static [&'static str],
    label: fn(Node, &[u8]) -> Option<String>,
}

// ---------------------------------------------------------------------------
// Per-language label extractors
// ---------------------------------------------------------------------------

/// Generic: read the `name` field as-is.
fn label_name(prefix: &str, node: Node, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    Some(format!("{prefix} {}", name.utf8_text(source).ok()?))
}

fn label_rust(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_item" => label_name("fn", node, source),
        "impl_item" => {
            let ty = node
                .child_by_field_name("type")?
                .utf8_text(source)
                .ok()?;
            if let Some(tr) = node.child_by_field_name("trait") {
                let tr = tr.utf8_text(source).ok()?;
                Some(format!("impl {tr} for {ty}"))
            } else {
                Some(format!("impl {ty}"))
            }
        }
        "struct_item" => label_name("struct", node, source),
        "enum_item" => label_name("enum", node, source),
        "trait_item" => label_name("trait", node, source),
        "mod_item" => label_name("mod", node, source),
        _ => None,
    }
}

fn label_python(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_definition" => label_name("def", node, source),
        "class_definition" => label_name("class", node, source),
        _ => None,
    }
}

fn label_javascript(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_declaration" => label_name("function", node, source),
        "class_declaration" => label_name("class", node, source),
        "method_definition" => label_name("method", node, source),
        _ => None,
    }
}

fn label_typescript(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_declaration" => label_name("function", node, source),
        "class_declaration" => label_name("class", node, source),
        "method_definition" => label_name("method", node, source),
        "interface_declaration" => label_name("interface", node, source),
        "type_alias_declaration" => label_name("type", node, source),
        _ => None,
    }
}

fn label_go(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_declaration" => label_name("func", node, source),
        "method_declaration" => label_name("func", node, source),
        "type_declaration" => {
            // type_declaration wraps type_spec which has the name
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_spec"
                    && let Some(name) = child.child_by_field_name("name")
                {
                    return Some(format!("type {}", name.utf8_text(source).ok()?));
                }
            }
            None
        }
        _ => None,
    }
}

fn label_ruby(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "method" => label_name("def", node, source),
        "singleton_method" => label_name("def self.", node, source),
        "class" => label_name("class", node, source),
        "module" => label_name("module", node, source),
        _ => None,
    }
}

fn label_java(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "class_declaration" => label_name("class", node, source),
        "interface_declaration" => label_name("interface", node, source),
        "method_declaration" => label_name("method", node, source),
        "constructor_declaration" => label_name("constructor", node, source),
        _ => None,
    }
}

fn label_c(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_definition" => {
            // name is nested: declarator (function_declarator) -> declarator (identifier)
            let decl = node.child_by_field_name("declarator")?;
            let name = decl
                .child_by_field_name("declarator")
                .and_then(|n| n.utf8_text(source).ok())
                .or_else(|| {
                    // fallback: first named child of declarator
                    let mut c = decl.walk();
                    decl.children(&mut c)
                        .find(|n| n.kind() == "identifier")
                        .and_then(|n| n.utf8_text(source).ok())
                })?;
            Some(format!("fn {name}"))
        }
        "struct_specifier" => label_name("struct", node, source),
        _ => None,
    }
}

fn label_cpp(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_definition" => {
            let decl = node.child_by_field_name("declarator")?;
            let name = decl
                .child_by_field_name("declarator")
                .and_then(|n| n.utf8_text(source).ok())
                .or_else(|| {
                    let mut c = decl.walk();
                    decl.children(&mut c)
                        .find(|n| n.kind() == "identifier" || n.kind() == "field_identifier")
                        .and_then(|n| n.utf8_text(source).ok())
                })?;
            Some(format!("fn {name}"))
        }
        "class_specifier" => label_name("class", node, source),
        "namespace_definition" => label_name("namespace", node, source),
        _ => None,
    }
}

fn label_bash(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_definition" => label_name("fn", node, source),
        _ => None,
    }
}

fn label_lua(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_declaration" => label_name("function", node, source),
        _ => None,
    }
}

fn label_css(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "rule_set" => {
            let sel = node.child_by_field_name("selectors")?;
            let text = sel.utf8_text(source).ok()?;
            Some(text.to_string())
        }
        "media_statement" => Some("@media".to_string()),
        _ => None,
    }
}

fn label_hcl(node: Node, source: &[u8]) -> Option<String> {
    if node.kind() != "block" {
        return None;
    }
    // HCL block: identifier followed by string_lit labels
    // e.g. resource "aws_instance" "example" { ... }
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                if let Ok(t) = child.utf8_text(source) {
                    parts.push(t.to_string());
                }
            }
            "string_lit" => {
                if let Ok(t) = child.utf8_text(source) {
                    // Strip quotes
                    let trimmed = t.trim_matches('"');
                    parts.push(trimmed.to_string());
                }
            }
            _ => break,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
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
            label: label_rust,
        }),
        "py" | "pyi" => Some(LangConfig {
            language: tree_sitter_python::LANGUAGE,
            scope_kinds: &["function_definition", "class_definition"],
            label: label_python,
        }),
        "js" | "mjs" | "cjs" | "jsx" => Some(LangConfig {
            language: tree_sitter_javascript::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
            label: label_javascript,
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
            label: label_typescript,
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
            label: label_typescript,
        }),
        "go" => Some(LangConfig {
            language: tree_sitter_go::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
            label: label_go,
        }),
        "rb" | "rake" | "gemspec" => Some(LangConfig {
            language: tree_sitter_ruby::LANGUAGE,
            scope_kinds: &["method", "singleton_method", "class", "module"],
            label: label_ruby,
        }),
        "java" => Some(LangConfig {
            language: tree_sitter_java::LANGUAGE,
            scope_kinds: &[
                "class_declaration",
                "interface_declaration",
                "method_declaration",
                "constructor_declaration",
            ],
            label: label_java,
        }),
        "c" | "h" => Some(LangConfig {
            language: tree_sitter_c::LANGUAGE,
            scope_kinds: &["function_definition", "struct_specifier"],
            label: label_c,
        }),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some(LangConfig {
            language: tree_sitter_cpp::LANGUAGE,
            scope_kinds: &[
                "function_definition",
                "class_specifier",
                "namespace_definition",
            ],
            label: label_cpp,
        }),
        "sh" | "bash" | "zsh" => Some(LangConfig {
            language: tree_sitter_bash::LANGUAGE,
            scope_kinds: &["function_definition"],
            label: label_bash,
        }),
        "lua" => Some(LangConfig {
            language: tree_sitter_lua::LANGUAGE,
            scope_kinds: &["function_declaration"],
            label: label_lua,
        }),
        "css" | "scss" => Some(LangConfig {
            language: tree_sitter_css::LANGUAGE,
            scope_kinds: &["rule_set", "media_statement"],
            label: label_css,
        }),
        "tf" | "hcl" => Some(LangConfig {
            language: tree_sitter_hcl::LANGUAGE,
            scope_kinds: &["block"],
            label: label_hcl,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Core algorithm
// ---------------------------------------------------------------------------

/// Find the enclosing scope chain for a given 0-indexed line.
fn enclosing_scope(
    root: Node,
    source: &[u8],
    line: usize,
    config: &LangConfig,
) -> Option<String> {
    let point = Point::new(line, 0);
    let node = root.descendant_for_point_range(point, point)?;

    let mut scopes = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        if config.scope_kinds.contains(&n.kind())
            && let Some(label) = (config.label)(n, source)
        {
            scopes.push(label);
        }
        current = n.parent();
    }
    scopes.reverse();
    if scopes.is_empty() {
        None
    } else {
        Some(scopes.join(" > "))
    }
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
///   `@@ -13,7 +13,7 @@ impl Config > fn compute`
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
    fn injects_rust_scope() {
        let original = "\
fn foo() {
    let x = 1;
    let y = 2;
}

fn bar() {
    let a = 1;
    let b = 2;
    let c = 3;
}
";
        let updated = original.replace("let b = 2", "let b = 99");
        let diff = raw_diff(original, &updated);
        let enriched = inject_scope_context(&diff, original, "test.rs");
        // The @@ line should now contain "fn bar"
        let hunk_line = enriched.lines().find(|l| l.starts_with("@@")).unwrap();
        assert!(
            hunk_line.contains("fn bar"),
            "expected 'fn bar' in hunk header, got: {hunk_line}"
        );
    }

    #[test]
    fn injects_rust_impl_scope() {
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
        assert!(
            hunk_line.contains("impl Foo > fn compute"),
            "expected nested scope, got: {hunk_line}"
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
            hunk_line.contains("class Calc") && hunk_line.contains("def sub"),
            "expected Python scope, got: {hunk_line}"
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
            hunk_line.contains("class Foo") && hunk_line.contains("method getValue"),
            "expected JS scope, got: {hunk_line}"
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
            hunk_line.contains("func hello"),
            "expected Go scope, got: {hunk_line}"
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
