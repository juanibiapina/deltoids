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
    /// Named code structures (functions, classes, modules, tables, headings).
    /// Resolution prefers the innermost structure that contains a change.
    structure_kinds: &'static [&'static str],
    /// Anonymous data containers (JSON/TS objects and arrays, YAML mappings
    /// and sequences). Used as a fallback when no structure wraps a change;
    /// resolution picks the outermost data container that still fits under
    /// `MAX_SCOPE_LINES`.
    data_kinds: &'static [&'static str],
    /// Wrapper kinds that should be promoted to a structure when their
    /// `value` field holds a function body. Used to give class fields and
    /// lexical declarations bound to arrow functions a proper hunk anchor
    /// and breadcrumb. A node only counts as a structure when its value
    /// child's kind appears in `function_body_kinds`.
    promoted_kinds: &'static [&'static str],
    /// Node kinds that introduce a function body. Used for two purposes:
    /// (1) promotion value check (is the wrapper's `value` a function?),
    /// and (2) nesting check (is this scope inside another function body,
    /// making it a local helper rather than an anchor?). Includes both
    /// expression forms (`arrow_function`, `function_expression`) and
    /// declaration forms (`function_declaration`, `method_definition`,
    /// `function_item`, ...).
    function_body_kinds: &'static [&'static str],
}

/// A parsed source file with its syntax tree and language metadata.
pub struct ParsedFile {
    pub tree: Tree,
    /// Node kinds used for the ancestor chain (display breadcrumb). Anchored
    /// with innermost strategy for hunk boundaries.
    pub structure_kinds: &'static [&'static str],
    /// Node kinds used as hunk anchors when no structure contains the change.
    /// Anchored with outermost-fit strategy.
    pub data_kinds: &'static [&'static str],
    /// Wrapper kinds promoted to structure when their `value` field is a
    /// function body. See `LangEntry::promoted_kinds`.
    pub promoted_kinds: &'static [&'static str],
    /// Node kinds that introduce a function body. See
    /// `LangEntry::function_body_kinds`.
    pub function_body_kinds: &'static [&'static str],
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
        structure_kinds: entry.structure_kinds,
        data_kinds: entry.data_kinds,
        promoted_kinds: entry.promoted_kinds,
        function_body_kinds: entry.function_body_kinds,
    })
}

// ---------------------------------------------------------------------------
// Language registry
// ---------------------------------------------------------------------------

/// Detect language from file extension and return its configuration.
#[allow(clippy::too_many_lines)]
fn detect_language(path: &str) -> Option<LangEntry> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(LangEntry {
            language: tree_sitter_rust::LANGUAGE,
            structure_kinds: &[
                "function_item",
                "impl_item",
                "struct_item",
                "enum_item",
                "trait_item",
                "mod_item",
            ],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_item", "closure_expression"],
        }),
        "py" | "pyi" => Some(LangEntry {
            language: tree_sitter_python::LANGUAGE,
            structure_kinds: &["function_definition", "class_definition"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_definition", "lambda"],
        }),
        "js" | "mjs" | "cjs" | "jsx" => Some(LangEntry {
            language: tree_sitter_javascript::LANGUAGE,
            structure_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
            data_kinds: &["object", "array"],
            // JS class fields use the kind `field_definition`. Lexical and
            // var declarations promote via their inner `variable_declarator`,
            // which directly carries the `name` and `value` fields.
            promoted_kinds: &["field_definition", "variable_declarator"],
            function_body_kinds: &[
                "function_declaration",
                "method_definition",
                "arrow_function",
                "function_expression",
                "function",
                "generator_function",
                "generator_function_declaration",
            ],
        }),
        "ts" | "mts" | "cts" => Some(LangEntry {
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            structure_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
            data_kinds: &["object", "array"],
            // TS class fields use `public_field_definition`. Lexical
            // declarations promote via their inner `variable_declarator`.
            promoted_kinds: &["public_field_definition", "variable_declarator"],
            function_body_kinds: &[
                "function_declaration",
                "method_definition",
                "arrow_function",
                "function_expression",
                "function",
                "generator_function",
                "generator_function_declaration",
            ],
        }),
        "tsx" => Some(LangEntry {
            language: tree_sitter_typescript::LANGUAGE_TSX,
            structure_kinds: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
            ],
            data_kinds: &["object", "array"],
            promoted_kinds: &["public_field_definition", "variable_declarator"],
            function_body_kinds: &[
                "function_declaration",
                "method_definition",
                "arrow_function",
                "function_expression",
                "function",
                "generator_function",
                "generator_function_declaration",
            ],
        }),
        "go" => Some(LangEntry {
            language: tree_sitter_go::LANGUAGE,
            structure_kinds: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_declaration", "method_declaration", "func_literal"],
        }),
        "rb" | "rake" | "gemspec" => Some(LangEntry {
            language: tree_sitter_ruby::LANGUAGE,
            structure_kinds: &["method", "singleton_method", "class", "module"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["method", "singleton_method", "block", "do_block", "lambda"],
        }),
        "java" => Some(LangEntry {
            language: tree_sitter_java::LANGUAGE,
            structure_kinds: &[
                "class_declaration",
                "interface_declaration",
                "method_declaration",
                "constructor_declaration",
            ],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &[
                "method_declaration",
                "constructor_declaration",
                "lambda_expression",
            ],
        }),
        "c" | "h" => Some(LangEntry {
            language: tree_sitter_c::LANGUAGE,
            structure_kinds: &["function_definition", "struct_specifier"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_definition"],
        }),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some(LangEntry {
            language: tree_sitter_cpp::LANGUAGE,
            structure_kinds: &[
                "function_definition",
                "class_specifier",
                "namespace_definition",
            ],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_definition", "lambda_expression"],
        }),
        "sh" | "bash" | "zsh" => Some(LangEntry {
            language: tree_sitter_bash::LANGUAGE,
            structure_kinds: &["function_definition"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_definition"],
        }),
        "lua" => Some(LangEntry {
            language: tree_sitter_lua::LANGUAGE,
            structure_kinds: &["function_declaration"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &["function_declaration", "function_definition"],
        }),
        "css" | "scss" => Some(LangEntry {
            language: tree_sitter_css::LANGUAGE,
            structure_kinds: &["rule_set", "media_statement"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &[],
        }),
        "tf" | "hcl" => Some(LangEntry {
            language: tree_sitter_hcl::LANGUAGE,
            structure_kinds: &["block"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &[],
        }),
        "md" | "markdown" => Some(LangEntry {
            language: tree_sitter_md::LANGUAGE,
            structure_kinds: &["atx_heading", "setext_heading"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &[],
        }),
        "toml" => Some(LangEntry {
            language: tree_sitter_toml_ng::LANGUAGE,
            structure_kinds: &["table", "table_array_element"],
            data_kinds: &[],
            promoted_kinds: &[],
            function_body_kinds: &[],
        }),
        "json" => Some(LangEntry {
            language: tree_sitter_json::LANGUAGE,
            structure_kinds: &[],
            data_kinds: &["object", "array"],
            promoted_kinds: &[],
            function_body_kinds: &[],
        }),
        "yaml" | "yml" => Some(LangEntry {
            language: tree_sitter_yaml::LANGUAGE,
            structure_kinds: &[],
            data_kinds: &["block_mapping", "block_sequence"],
            promoted_kinds: &[],
            function_body_kinds: &[],
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
        assert!(parsed.structure_kinds.contains(&"function_item"));
        assert!(parsed.data_kinds.is_empty());
    }

    #[test]
    fn parses_python_file() {
        let source = "def hello():\n    pass\n";
        let parsed = parse_file("app.py", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "module");
        assert!(parsed.structure_kinds.contains(&"function_definition"));
    }

    #[test]
    fn parses_markdown_file() {
        let source = "# Heading\n\nSome text.\n";
        let parsed = parse_file("README.md", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "document");
        assert!(parsed.structure_kinds.contains(&"atx_heading"));
    }

    #[test]
    fn parses_toml_file() {
        let source = "[package]\nname = \"test\"\n";
        let parsed = parse_file("Cargo.toml", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "document");
        assert!(parsed.structure_kinds.contains(&"table"));
    }

    #[test]
    fn parses_json_file() {
        let source = "{\"name\": \"test\", \"version\": \"1.0\"}\n";
        let parsed = parse_file("package.json", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "document");
        assert!(parsed.structure_kinds.is_empty());
        assert!(parsed.data_kinds.contains(&"object"));
    }

    #[test]
    fn parses_yaml_file() {
        let source = "name: test\nversion: 1.0\n";
        let parsed = parse_file("config.yaml", source).unwrap();
        assert_eq!(parsed.tree.root_node().kind(), "stream");
        assert!(parsed.structure_kinds.is_empty());
        assert!(parsed.data_kinds.contains(&"block_mapping"));
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
            ("test.md", "# Heading"),
            ("test.toml", "[section]"),
            ("test.json", "{\"key\": \"value\"}"),
            ("test.yaml", "key: value"),
            ("test.yml", "key: value"),
        ];
        for (path, source) in cases {
            assert!(parse_file(path, source).is_some(), "failed to parse {path}");
        }
    }
}
