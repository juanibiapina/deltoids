//! Tree-sitter language detection and parsing.
//!
//! Detects the programming language from a file path and parses source code
//! into a tree-sitter syntax tree. This module provides the shared foundation
//! for all tree-sitter based features: scope context, breadcrumbs, sibling
//! folding, and change classification.

use std::path::Path;

use tree_sitter::{Parser, Tree};
use tree_sitter_language::LanguageFn;

/// Per-language tree-sitter configuration.
struct LangEntry {
    language: LanguageFn,
    scope_kinds: &'static [&'static str],
}

/// A parsed source file with its syntax tree and language metadata.
pub struct ParsedFile {
    pub tree: Tree,
    /// Node kinds that represent scope boundaries (functions, classes, etc.).
    pub scope_kinds: &'static [&'static str],
}

/// Detect the language from a file path and parse the source text.
///
/// Returns `None` if the language is not recognized or parsing fails.
pub fn parse_file(path: &str, source: &str) -> Option<ParsedFile> {
    let entry = detect_language(path)?;
    let mut parser = Parser::new();
    parser.set_language(&entry.language.into()).ok()?;
    let tree = parser.parse(source, None)?;
    Some(ParsedFile {
        tree,
        scope_kinds: entry.scope_kinds,
    })
}

// ---------------------------------------------------------------------------
// Language registry
// ---------------------------------------------------------------------------

/// Detect language from file extension and return its configuration.
fn detect_language(path: &str) -> Option<LangEntry> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(LangEntry {
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
        "py" | "pyi" => Some(LangEntry {
            language: tree_sitter_python::LANGUAGE,
            scope_kinds: &["function_definition", "class_definition"],
        }),
        "js" | "mjs" | "cjs" | "jsx" => Some(LangEntry {
            language: tree_sitter_javascript::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
        }),
        "ts" | "mts" | "cts" => Some(LangEntry {
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
        }),
        "tsx" => Some(LangEntry {
            language: tree_sitter_typescript::LANGUAGE_TSX,
            scope_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
        }),
        "go" => Some(LangEntry {
            language: tree_sitter_go::LANGUAGE,
            scope_kinds: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        }),
        "rb" | "rake" | "gemspec" => Some(LangEntry {
            language: tree_sitter_ruby::LANGUAGE,
            scope_kinds: &["method", "singleton_method", "class", "module"],
        }),
        "java" => Some(LangEntry {
            language: tree_sitter_java::LANGUAGE,
            scope_kinds: &[
                "class_declaration",
                "interface_declaration",
                "method_declaration",
                "constructor_declaration",
            ],
        }),
        "c" | "h" => Some(LangEntry {
            language: tree_sitter_c::LANGUAGE,
            scope_kinds: &["function_definition", "struct_specifier"],
        }),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some(LangEntry {
            language: tree_sitter_cpp::LANGUAGE,
            scope_kinds: &[
                "function_definition",
                "class_specifier",
                "namespace_definition",
            ],
        }),
        "sh" | "bash" | "zsh" => Some(LangEntry {
            language: tree_sitter_bash::LANGUAGE,
            scope_kinds: &["function_definition"],
        }),
        "lua" => Some(LangEntry {
            language: tree_sitter_lua::LANGUAGE,
            scope_kinds: &["function_declaration"],
        }),
        "css" | "scss" => Some(LangEntry {
            language: tree_sitter_css::LANGUAGE,
            scope_kinds: &["rule_set", "media_statement"],
        }),
        "tf" | "hcl" => Some(LangEntry {
            language: tree_sitter_hcl::LANGUAGE,
            scope_kinds: &["block"],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rust_file() {
        let source = "fn main() { let x = 1; }\n";
        let parsed = parse_file("src/main.rs", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "source_file");
        assert!(parsed.scope_kinds.contains(&"function_item"));
    }

    #[test]
    fn parses_python_file() {
        let source = "def hello():\n    pass\n";
        let parsed = parse_file("app.py", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "module");
        assert!(parsed.scope_kinds.contains(&"function_definition"));
    }

    #[test]
    fn returns_none_for_unknown_extension() {
        assert!(parse_file("data.xyz", "content").is_none());
    }

    #[test]
    fn returns_none_for_no_extension() {
        assert!(parse_file("Makefile", "all: build").is_none());
    }

    #[test]
    fn detects_all_supported_languages() {
        let cases = vec![
            ("test.rs", "fn main() {}"),
            ("test.py", "def f(): pass"),
            ("test.js", "function f() {}"),
            ("test.ts", "function f() {}"),
            ("test.tsx", "function f() {}"),
            ("test.go", "package main"),
            ("test.rb", "def f; end"),
            ("test.java", "class A {}"),
            ("test.c", "int main() {}"),
            ("test.cpp", "int main() {}"),
            ("test.sh", "echo hi"),
            ("test.lua", "print('hi')"),
            ("test.css", "body {}"),
            ("test.tf", "resource {}"),
        ];
        for (path, source) in cases {
            assert!(parse_file(path, source).is_some(), "failed to parse {path}");
        }
    }
}
