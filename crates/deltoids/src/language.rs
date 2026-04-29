//! Stable language detection and language-specific configuration.
//!
//! Detection uses the syntax definitions bundled in the binary, not user bat
//! cache, so scope extraction is stable across machines. Rendering can still
//! use user-loaded bat assets for themes and custom syntaxes.

use std::path::Path;
use std::sync::OnceLock;

use bat::assets::HighlightingAssets;
use serde::{Deserialize, Serialize};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use tree_sitter_language::LanguageFn;

/// Programming languages supported by deltoids' tree-sitter scope engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Bash,
    C,
    Cpp,
    Css,
    Go,
    Hcl,
    Java,
    JavaScript,
    Json,
    Lua,
    Markdown,
    Python,
    Ruby,
    Rust,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
}

/// Per-language tree-sitter configuration.
pub(crate) struct TreeSitterConfig {
    pub(crate) language: LanguageFn,
    /// Named code structures (functions, classes, modules, tables, headings).
    /// Resolution prefers the innermost structure that contains a change.
    pub(crate) structure_kinds: &'static [&'static str],
    /// Anonymous data containers (JSON/TS objects and arrays, YAML mappings
    /// and sequences). Used as a fallback when no structure wraps a change;
    /// resolution picks the outermost data container that still fits under
    /// `MAX_SCOPE_LINES`.
    pub(crate) data_kinds: &'static [&'static str],
    /// Wrapper kinds that should be promoted to a structure when their
    /// `value` field holds a function body.
    pub(crate) promoted_kinds: &'static [&'static str],
    /// Node kinds that introduce a function body. Used for promotion checks
    /// and for demoting local helpers nested inside another function body.
    pub(crate) function_body_kinds: &'static [&'static str],
}

impl Language {
    /// Stable identifier for persisted/configured language values.
    pub fn id(self) -> &'static str {
        match self {
            Language::Bash => "bash",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Css => "css",
            Language::Go => "go",
            Language::Hcl => "hcl",
            Language::Java => "java",
            Language::JavaScript => "javascript",
            Language::Json => "json",
            Language::Lua => "lua",
            Language::Markdown => "markdown",
            Language::Python => "python",
            Language::Ruby => "ruby",
            Language::Rust => "rust",
            Language::Toml => "toml",
            Language::Tsx => "tsx",
            Language::TypeScript => "typescript",
            Language::Yaml => "yaml",
        }
    }

    /// Parse a stable language identifier.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "bash" => Some(Language::Bash),
            "c" => Some(Language::C),
            "cpp" => Some(Language::Cpp),
            "css" => Some(Language::Css),
            "go" => Some(Language::Go),
            "hcl" => Some(Language::Hcl),
            "java" => Some(Language::Java),
            "javascript" => Some(Language::JavaScript),
            "json" => Some(Language::Json),
            "lua" => Some(Language::Lua),
            "markdown" => Some(Language::Markdown),
            "python" => Some(Language::Python),
            "ruby" => Some(Language::Ruby),
            "rust" => Some(Language::Rust),
            "toml" => Some(Language::Toml),
            "tsx" => Some(Language::Tsx),
            "typescript" => Some(Language::TypeScript),
            "yaml" => Some(Language::Yaml),
            _ => None,
        }
    }

    /// Detect the language for an in-memory file snapshot.
    ///
    /// Tries file name, extension, then the first source line for shebangs and
    /// modelines. The path is never read from disk.
    pub(crate) fn detect(path: &str, source: &str) -> Option<Self> {
        let syntax_set = detection_syntax_set();
        detect_syntax_by_path(syntax_set, path)
            .or_else(|| detect_syntax_by_first_line(syntax_set, source))
            .and_then(Self::from_syntax)
    }

    /// Token used to find a syntax in syntect sets for rendering.
    pub(crate) fn syntax_token(self) -> &'static str {
        match self {
            Language::Bash => "bash",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Css => "css",
            Language::Go => "go",
            Language::Hcl => "hcl",
            Language::Java => "java",
            Language::JavaScript => "js",
            Language::Json => "json",
            Language::Lua => "lua",
            Language::Markdown => "md",
            Language::Python => "py",
            Language::Ruby => "rb",
            Language::Rust => "rs",
            Language::Toml => "toml",
            Language::Tsx => "tsx",
            Language::TypeScript => "ts",
            Language::Yaml => "yaml",
        }
    }

    /// Tree-sitter parser and scope taxonomy for this language.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn tree_sitter_config(self) -> TreeSitterConfig {
        match self {
            Language::Rust => TreeSitterConfig {
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
            },
            Language::Python => TreeSitterConfig {
                language: tree_sitter_python::LANGUAGE,
                structure_kinds: &["function_definition", "class_definition"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition", "lambda"],
            },
            Language::JavaScript => TreeSitterConfig {
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
            },
            Language::TypeScript => TreeSitterConfig {
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
            },
            Language::Tsx => TreeSitterConfig {
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
            },
            Language::Go => TreeSitterConfig {
                language: tree_sitter_go::LANGUAGE,
                structure_kinds: &[
                    "function_declaration",
                    "method_declaration",
                    "type_declaration",
                ],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[
                    "function_declaration",
                    "method_declaration",
                    "func_literal",
                ],
            },
            Language::Ruby => TreeSitterConfig {
                language: tree_sitter_ruby::LANGUAGE,
                structure_kinds: &["method", "singleton_method", "class", "module"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["method", "singleton_method", "block", "do_block", "lambda"],
            },
            Language::Java => TreeSitterConfig {
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
            },
            Language::C => TreeSitterConfig {
                language: tree_sitter_c::LANGUAGE,
                structure_kinds: &["function_definition", "struct_specifier"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition"],
            },
            Language::Cpp => TreeSitterConfig {
                language: tree_sitter_cpp::LANGUAGE,
                structure_kinds: &[
                    "function_definition",
                    "class_specifier",
                    "namespace_definition",
                ],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition", "lambda_expression"],
            },
            Language::Bash => TreeSitterConfig {
                language: tree_sitter_bash::LANGUAGE,
                structure_kinds: &["function_definition"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition"],
            },
            Language::Lua => TreeSitterConfig {
                language: tree_sitter_lua::LANGUAGE,
                structure_kinds: &["function_declaration"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &["function_declaration", "function_definition"],
            },
            Language::Css => TreeSitterConfig {
                language: tree_sitter_css::LANGUAGE,
                structure_kinds: &["rule_set", "media_statement"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
            Language::Hcl => TreeSitterConfig {
                language: tree_sitter_hcl::LANGUAGE,
                structure_kinds: &["block"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
            Language::Markdown => TreeSitterConfig {
                language: tree_sitter_md::LANGUAGE,
                structure_kinds: &["atx_heading", "setext_heading"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
            Language::Toml => TreeSitterConfig {
                language: tree_sitter_toml_ng::LANGUAGE,
                structure_kinds: &["table", "table_array_element"],
                data_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
            Language::Json => TreeSitterConfig {
                language: tree_sitter_json::LANGUAGE,
                structure_kinds: &[],
                data_kinds: &["object", "array"],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
            Language::Yaml => TreeSitterConfig {
                language: tree_sitter_yaml::LANGUAGE,
                structure_kinds: &[],
                data_kinds: &["block_mapping", "block_sequence"],
                promoted_kinds: &[],
                function_body_kinds: &[],
            },
        }
    }

    fn from_syntax(syntax: &SyntaxReference) -> Option<Self> {
        let name = syntax.name.as_str();
        if name.starts_with("Bourne Again Shell") {
            return Some(Language::Bash);
        }
        if name.starts_with("JavaScript") {
            return Some(Language::JavaScript);
        }

        match name {
            "C" => Some(Language::C),
            "C++" => Some(Language::Cpp),
            "CSS" | "SCSS" => Some(Language::Css),
            "Go" => Some(Language::Go),
            "HCL" | "Terraform" => Some(Language::Hcl),
            "Java" => Some(Language::Java),
            "JSON" => Some(Language::Json),
            "Lua" => Some(Language::Lua),
            "Markdown" => Some(Language::Markdown),
            "Python" => Some(Language::Python),
            "Ruby" => Some(Language::Ruby),
            "Rust" => Some(Language::Rust),
            "TOML" => Some(Language::Toml),
            "TypeScript" => Some(Language::TypeScript),
            "TypeScriptReact" => Some(Language::Tsx),
            "YAML" => Some(Language::Yaml),
            _ => None,
        }
    }
}

static DETECTION_SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

fn detection_syntax_set() -> &'static SyntaxSet {
    DETECTION_SYNTAX_SET.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_syntax_set()
            .expect("bundled syntax assets should load")
            .clone()
    })
}

fn detect_syntax_by_path<'a>(syntax_set: &'a SyntaxSet, path: &str) -> Option<&'a SyntaxReference> {
    let path = Path::new(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    syntax_set
        .find_syntax_by_extension(file_name)
        .or_else(|| syntax_set.find_syntax_by_extension(extension))
}

fn detect_syntax_by_first_line<'a>(
    syntax_set: &'a SyntaxSet,
    source: &str,
) -> Option<&'a SyntaxReference> {
    let first_line = source.lines().next().unwrap_or("");
    syntax_set.find_syntax_by_first_line(first_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ruby_from_exact_filename() {
        assert_eq!(
            Language::detect("Gemfile", "source 'https://rubygems.org'\n").map(Language::id),
            Some("ruby")
        );
    }

    #[test]
    fn detects_python_from_shebang() {
        assert_eq!(
            Language::detect("script", "#!/usr/bin/env python3\nprint('hi')\n").map(Language::id),
            Some("python")
        );
    }

    #[test]
    fn detects_bash_from_shebang() {
        assert_eq!(
            Language::detect("script", "#!/usr/bin/env bash\nset -e\n").map(Language::id),
            Some("bash")
        );
    }

    #[test]
    fn ids_round_trip() {
        for language in [
            Language::Bash,
            Language::C,
            Language::Cpp,
            Language::Css,
            Language::Go,
            Language::Hcl,
            Language::Java,
            Language::JavaScript,
            Language::Json,
            Language::Lua,
            Language::Markdown,
            Language::Python,
            Language::Ruby,
            Language::Rust,
            Language::Toml,
            Language::Tsx,
            Language::TypeScript,
            Language::Yaml,
        ] {
            assert_eq!(Language::from_id(language.id()), Some(language));
        }
    }
}
