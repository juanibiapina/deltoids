//! Stable language detection and language-specific configuration.
//!
//! Detection uses the syntax definitions bundled in the binary, so scope
//! extraction is stable across machines.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use tree_sitter_language::LanguageFn;

use crate::definitions::DefinitionKind;

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
    Kotlin,
    Lua,
    Markdown,
    Python,
    Ruby,
    Rust,
    Sql,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
}

/// How a scope kind is named when its node has no plain name field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameRule {
    /// Leading `identifier` / `string_lit` children joined with spaces:
    /// HCL blocks, `resource "aws_s3_bucket" "logs"`.
    Positional,
    /// The nested `object_reference`: SQL DDL, `users`, `public.users`.
    ObjectReference,
    /// The name at the end of the declarator chain, as written: C and C++
    /// functions, `top`, `Foo::baz`, `Foo::~Foo`, `Foo::operator==`.
    Declarator,
    /// `Trait for Type`, or `Type` for an inherent impl: Rust impl blocks.
    ImplHeader,
    /// `Type.Method`, the receiver type without pointer or type
    /// arguments: Go methods, `Server.Start`.
    Receiver,
    /// `receiver.name`: Ruby singleton methods, `self.build`.
    SingletonMethod,
    /// The names of the types it declares, joined with `, `: Go `type`
    /// declarations.
    Specs,
    /// The text before the node's `block`: CSS rules, `.a, .b > p`, and
    /// at-rules, `@media (max-width: 600px)`.
    Header,
    /// The first child key as written: TOML tables and pairs,
    /// `dependencies.serde`.
    TableKey,
    /// The heading's text: Markdown headings.
    HeadingText,
    /// The text of the heading that opens it: Markdown sections.
    SectionHeading,
    /// The declared variable's name: Kotlin properties.
    Property,
    /// The text of the first child of this kind: Lua `variable_list`,
    /// HCL attribute `identifier`.
    ChildKind(&'static str),
    /// The `key` field without its quotes: JSON and YAML keys.
    QuotedKey,
    /// The text of this field: the `left` side of a Python or Ruby
    /// assignment.
    Field(&'static str),
}

/// A kind of definition that is not a scope: a constant, a type alias, a
/// field, a key. Scope kinds are definitions through
/// [`TreeSitterConfig::definition_kinds`] and the breadcrumb rules.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LeafDefinition {
    pub(crate) kind: &'static str,
    pub(crate) category: DefinitionKind,
    /// Node kinds allowed between the definition and its container: the
    /// nearest enclosing definition, or the file root. A JS `const` may sit
    /// in a `lexical_declaration` behind an `export`; one inside an `if`
    /// block is a statement, not a definition.
    pub(crate) through: &'static [&'static str],
    pub(crate) require: Require,
    /// How many enclosing definitions of this same kind it may have:
    /// JSON and YAML list keys two levels deep. `None` for any.
    pub(crate) levels: Option<usize>,
}

/// What a node needs, besides its kind, to be a [`LeafDefinition`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Require {
    Nothing,
    /// A `function_declarator` in its declarator chain: C and C++
    /// prototypes and method declarations.
    FunctionDeclarator,
    /// A `body` field: a C `enum` or `struct` that defines its members.
    Body,
    /// A `left` field of one of these kinds: Python and Ruby assignments.
    Left(&'static [&'static str]),
    /// A function as its value: Lua `M.f = function() … end`.
    FunctionValue,
}

const fn leaf(
    kind: &'static str,
    category: DefinitionKind,
    through: &'static [&'static str],
) -> LeafDefinition {
    LeafDefinition {
        kind,
        category,
        through,
        require: Require::Nothing,
        levels: None,
    }
}

const fn leaf_if(
    kind: &'static str,
    category: DefinitionKind,
    through: &'static [&'static str],
    require: Require,
) -> LeafDefinition {
    LeafDefinition {
        kind,
        category,
        through,
        require,
        levels: None,
    }
}

const fn key(kind: &'static str, through: &'static [&'static str]) -> LeafDefinition {
    LeafDefinition {
        kind,
        category: DefinitionKind::Key,
        through,
        require: Require::Nothing,
        levels: Some(1),
    }
}

/// Preprocessor conditionals a C or C++ definition may sit in (include
/// guards wrap a whole header).
const C_PREPROCESSOR: &[&str] = &[
    "preproc_ifdef",
    "preproc_if",
    "preproc_else",
    "preproc_elif",
    "preproc_elifdef",
];

const RUST_LEAVES: &[LeafDefinition] = &[
    leaf(
        "function_signature_item",
        DefinitionKind::Function,
        &["declaration_list"],
    ),
    leaf(
        "const_item",
        DefinitionKind::Constant,
        &["declaration_list"],
    ),
    leaf(
        "static_item",
        DefinitionKind::Constant,
        &["declaration_list"],
    ),
    leaf(
        "type_item",
        DefinitionKind::TypeAlias,
        &["declaration_list"],
    ),
    leaf("union_item", DefinitionKind::Union, &["declaration_list"]),
    leaf(
        "macro_definition",
        DefinitionKind::Macro,
        &["declaration_list"],
    ),
];

const PYTHON_LEAVES: &[LeafDefinition] = &[leaf_if(
    "assignment",
    DefinitionKind::Variable,
    &["expression_statement", "block"],
    Require::Left(&["identifier"]),
)];

const JAVASCRIPT_LEAVES: &[LeafDefinition] = &[
    leaf(
        "generator_function_declaration",
        DefinitionKind::Function,
        &["export_statement"],
    ),
    leaf(
        "variable_declarator",
        DefinitionKind::Variable,
        &[
            "lexical_declaration",
            "variable_declaration",
            "export_statement",
        ],
    ),
    leaf(
        "field_definition",
        DefinitionKind::Variable,
        &["class_body"],
    ),
];

const TYPESCRIPT_LEAVES: &[LeafDefinition] = &[
    leaf(
        "generator_function_declaration",
        DefinitionKind::Function,
        &["export_statement"],
    ),
    leaf(
        "variable_declarator",
        DefinitionKind::Variable,
        &[
            "lexical_declaration",
            "variable_declaration",
            "export_statement",
        ],
    ),
    leaf(
        "public_field_definition",
        DefinitionKind::Variable,
        &["class_body"],
    ),
    leaf(
        "enum_declaration",
        DefinitionKind::Enum,
        &["export_statement"],
    ),
    leaf(
        "abstract_class_declaration",
        DefinitionKind::Class,
        &["export_statement"],
    ),
    leaf(
        "internal_module",
        DefinitionKind::Namespace,
        &["expression_statement", "export_statement"],
    ),
    leaf(
        "function_signature",
        DefinitionKind::Function,
        &["export_statement"],
    ),
    leaf(
        "method_signature",
        DefinitionKind::Method,
        &["interface_body", "object_type"],
    ),
    leaf(
        "abstract_method_signature",
        DefinitionKind::Method,
        &["class_body"],
    ),
];

const GO_LEAVES: &[LeafDefinition] = &[
    leaf(
        "const_spec",
        DefinitionKind::Constant,
        &["const_declaration"],
    ),
    leaf("var_spec", DefinitionKind::Variable, &["var_declaration"]),
];

const RUBY_LEAVES: &[LeafDefinition] = &[leaf_if(
    "assignment",
    DefinitionKind::Constant,
    &["body_statement"],
    Require::Left(&["constant"]),
)];

const KOTLIN_LEAVES: &[LeafDefinition] = &[
    leaf(
        "property_declaration",
        DefinitionKind::Variable,
        &["class_body"],
    ),
    leaf("type_alias", DefinitionKind::TypeAlias, &[]),
    leaf("companion_object", DefinitionKind::Object, &["class_body"]),
];

const JAVA_LEAVES: &[LeafDefinition] = &[
    leaf(
        "enum_declaration",
        DefinitionKind::Enum,
        &["class_body", "interface_body"],
    ),
    leaf(
        "record_declaration",
        DefinitionKind::Class,
        &["class_body", "interface_body"],
    ),
    leaf(
        "annotation_type_declaration",
        DefinitionKind::Interface,
        &["class_body", "interface_body"],
    ),
    leaf(
        "field_declaration",
        DefinitionKind::Variable,
        &["class_body"],
    ),
];

const C_LEAVES: &[LeafDefinition] = &[
    leaf_if(
        "declaration",
        DefinitionKind::Function,
        &[
            "linkage_specification",
            "declaration_list",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::FunctionDeclarator,
    ),
    leaf("type_definition", DefinitionKind::TypeAlias, C_PREPROCESSOR),
    leaf_if(
        "enum_specifier",
        DefinitionKind::Enum,
        &[
            "declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::Body,
    ),
    leaf_if(
        "union_specifier",
        DefinitionKind::Union,
        &[
            "declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::Body,
    ),
    leaf("preproc_def", DefinitionKind::Macro, C_PREPROCESSOR),
    leaf(
        "preproc_function_def",
        DefinitionKind::Macro,
        C_PREPROCESSOR,
    ),
];

const CPP_LEAVES: &[LeafDefinition] = &[
    leaf_if(
        "declaration",
        DefinitionKind::Function,
        &[
            "declaration_list",
            "linkage_specification",
            "template_declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::FunctionDeclarator,
    ),
    leaf_if(
        "field_declaration",
        DefinitionKind::Method,
        &[
            "field_declaration_list",
            "template_declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::FunctionDeclarator,
    ),
    leaf(
        "alias_declaration",
        DefinitionKind::TypeAlias,
        &[
            "declaration_list",
            "field_declaration_list",
            "template_declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
    ),
    leaf(
        "type_definition",
        DefinitionKind::TypeAlias,
        &[
            "declaration_list",
            "field_declaration_list",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
    ),
    leaf_if(
        "struct_specifier",
        DefinitionKind::Struct,
        &[
            "declaration",
            "declaration_list",
            "field_declaration_list",
            "template_declaration",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::Body,
    ),
    leaf_if(
        "enum_specifier",
        DefinitionKind::Enum,
        &[
            "declaration",
            "declaration_list",
            "field_declaration_list",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::Body,
    ),
    leaf_if(
        "union_specifier",
        DefinitionKind::Union,
        &[
            "declaration",
            "declaration_list",
            "field_declaration_list",
            "preproc_ifdef",
            "preproc_if",
            "preproc_else",
            "preproc_elif",
            "preproc_elifdef",
        ],
        Require::Body,
    ),
    leaf("preproc_def", DefinitionKind::Macro, C_PREPROCESSOR),
    leaf(
        "preproc_function_def",
        DefinitionKind::Macro,
        C_PREPROCESSOR,
    ),
];

const LUA_LEAVES: &[LeafDefinition] = &[leaf_if(
    "assignment_statement",
    DefinitionKind::Function,
    &["variable_declaration"],
    Require::FunctionValue,
)];

const CSS_LEAVES: &[LeafDefinition] = &[
    leaf("keyframes_statement", DefinitionKind::Block, &[]),
    leaf("supports_statement", DefinitionKind::Block, &[]),
];

const HCL_LEAVES: &[LeafDefinition] = &[leaf("attribute", DefinitionKind::Variable, &["body"])];

const MARKDOWN_LEAVES: &[LeafDefinition] = &[leaf("section", DefinitionKind::Section, &[])];

const TOML_LEAVES: &[LeafDefinition] = &[leaf("pair", DefinitionKind::Key, &[])];

const JSON_LEAVES: &[LeafDefinition] = &[key("pair", &["object"])];

const YAML_LEAVES: &[LeafDefinition] = &[key(
    "block_mapping_pair",
    &["document", "block_node", "block_mapping"],
)];

/// How a language says who can use a definition; see
/// `crate::syntax::visibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisibilityRule {
    /// The language has no visibility.
    None,
    /// `pub`, `pub(crate)`, or nothing; `#[macro_export]` for macros.
    Rust,
    /// `export` at the top of a module; `private`, `protected`, `#name`
    /// on class members: JavaScript and TypeScript.
    Exported,
    /// A capitalized name is exported: Go.
    Capitalized,
    /// A name starting with `_` is private, a dunder is not: Python.
    Underscore,
}

/// Per-language tree-sitter configuration.
pub(crate) struct TreeSitterConfig {
    /// Named code structures (functions, classes, modules, tables, headings).
    /// Resolution prefers the innermost structure that contains a change.
    pub(crate) structure_kinds: &'static [&'static str],
    /// Wrapper kinds that should be promoted to a structure when their
    /// `value` field holds a function body.
    pub(crate) promoted_kinds: &'static [&'static str],
    /// Node kinds that introduce a function body. Used for promotion checks
    /// and for demoting local helpers nested inside another function body.
    pub(crate) function_body_kinds: &'static [&'static str],
    /// Anonymous function-body kinds that act as hunk anchors but never
    /// appear in the breadcrumb. Use for callbacks like `arrow_function`
    /// in JS/TS where the call signature on the opening line carries the
    /// label (e.g. `it("name", () => {`) and a synthetic `[KIND name]`
    /// breadcrumb entry would just be noise.
    pub(crate) anchor_only_kinds: &'static [&'static str],
    /// Wrapper kinds promoted to a *named* structure when one of their
    /// arguments is a function body. Use for `call_expression` in JS/TS
    /// so labeled callbacks (`describe("…", () => {})`,
    /// `it("…", () => {})`, `app.get("/…", (req, res) => {})`) appear
    /// in the breadcrumb with a real name (callee + first string-literal
    /// arg) even when the inner arrow function is too large to size the
    /// hunk.
    pub(crate) call_promoted_kinds: &'static [&'static str],
    /// How kinds whose name is not in a `name`, `property`, `type`, or
    /// `key` field are named; see [`NameRule`]. Kinds not listed read
    /// those fields, in that order.
    pub(crate) name_rules: &'static [(&'static str, NameRule)],
    /// The category of each scope kind that is a definition. Scope kinds
    /// missing here (anonymous callbacks, Markdown headings) are never
    /// definitions.
    pub(crate) definition_kinds: &'static [(&'static str, DefinitionKind)],
    /// Definitions that are not scopes.
    pub(crate) leaf_definitions: &'static [LeafDefinition],
    /// Child kinds a definition's body starts at when it has no `body`
    /// field: Kotlin `function_body`, CSS `block`, TOML values.
    pub(crate) body_kinds: &'static [&'static str],
    /// Kinds that import other code: `use`, `import`, `#include`.
    pub(crate) import_kinds: &'static [&'static str],
    /// Node kinds that, when they appear as siblings above a structure,
    /// attach to that structure for scope queries. Typically `comment`
    /// plus the language's attribute kinds (Rust `attribute_item`).
    /// Rust inner doc comments (`//!`, `/*!`) never attach, though they
    /// share the comment kinds: they document the enclosing module.
    /// Mirrors the role `decorator` plays for `skip_decorators`.
    pub(crate) leading_comment_kinds: &'static [&'static str],
    /// Whether a contained change with no enclosing structure may expand
    /// outward through transparent ancestors (see
    /// [`crate::syntax::ParsedFile::expansion_anchor`]). True for code and
    /// data languages, where the outermost fitting ancestor is a single
    /// statement or a compact container. False for prose (Markdown), where
    /// the outermost fitting ancestor is a whole heading-delimited
    /// `section` that would drag in unrelated sibling blocks; such changes
    /// fall back to 3-line default context until a Markdown-specific
    /// section model exists.
    pub(crate) transparent_expansion: bool,
    /// How the language says who can use a definition.
    pub(crate) visibility: VisibilityRule,
}

impl TreeSitterConfig {
    /// How `kind` is named, when it has a rule.
    pub(crate) fn name_rule(&self, kind: &str) -> Option<NameRule> {
        self.name_rules
            .iter()
            .find(|(rule_kind, _)| *rule_kind == kind)
            .map(|(_, rule)| *rule)
    }
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
            Language::Kotlin => "kotlin",
            Language::Lua => "lua",
            Language::Markdown => "markdown",
            Language::Python => "python",
            Language::Ruby => "ruby",
            Language::Rust => "rust",
            Language::Sql => "sql",
            Language::Toml => "toml",
            Language::Tsx => "tsx",
            Language::TypeScript => "typescript",
            Language::Yaml => "yaml",
        }
    }

    /// Human-readable name for display.
    pub fn name(self) -> &'static str {
        match self {
            Language::Bash => "Bash",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Css => "CSS",
            Language::Go => "Go",
            Language::Hcl => "HCL",
            Language::Java => "Java",
            Language::JavaScript => "JavaScript",
            Language::Json => "JSON",
            Language::Kotlin => "Kotlin",
            Language::Lua => "Lua",
            Language::Markdown => "Markdown",
            Language::Python => "Python",
            Language::Ruby => "Ruby",
            Language::Rust => "Rust",
            Language::Sql => "SQL",
            Language::Toml => "TOML",
            Language::Tsx => "TSX",
            Language::TypeScript => "TypeScript",
            Language::Yaml => "YAML",
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
            "kotlin" => Some(Language::Kotlin),
            "lua" => Some(Language::Lua),
            "markdown" => Some(Language::Markdown),
            "python" => Some(Language::Python),
            "ruby" => Some(Language::Ruby),
            "rust" => Some(Language::Rust),
            "sql" => Some(Language::Sql),
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
    /// modelines. The path is never read from disk. A language whose grammar
    /// is left out is not detected.
    pub(crate) fn detect(path: &str, source: &str) -> Option<Self> {
        let syntax_set = detection_syntax_set();
        detect_syntax_by_path(syntax_set, path)
            .or_else(|| detect_syntax_by_first_line(syntax_set, source))
            .and_then(Self::from_syntax)
            .filter(|language| language.grammar().is_some())
    }

    /// Detect the syntect syntax name (the highlight key), independent of
    /// tree-sitter support. The path is never read from disk.
    pub fn detect_highlight_name(path: &str, source: &str) -> Option<String> {
        let syntax_set = detection_syntax_set();
        detect_syntax_by_path(syntax_set, path)
            .or_else(|| detect_syntax_by_first_line(syntax_set, source))
            .map(|syntax| syntax.name.clone())
    }

    /// The tree-sitter grammar for this language, when its `lang-*` feature is on.
    #[allow(unreachable_patterns, clippy::match_single_binding)]
    pub(crate) fn grammar(self) -> Option<LanguageFn> {
        match self {
            #[cfg(feature = "lang-rust")]
            Language::Rust => Some(tree_sitter_rust::LANGUAGE),
            #[cfg(feature = "lang-python")]
            Language::Python => Some(tree_sitter_python::LANGUAGE),
            #[cfg(feature = "lang-javascript")]
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE),
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            #[cfg(feature = "lang-typescript")]
            Language::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX),
            #[cfg(feature = "lang-go")]
            Language::Go => Some(tree_sitter_go::LANGUAGE),
            #[cfg(feature = "lang-ruby")]
            Language::Ruby => Some(tree_sitter_ruby::LANGUAGE),
            #[cfg(feature = "lang-kotlin")]
            Language::Kotlin => Some(tree_sitter_kotlin_ng::LANGUAGE),
            #[cfg(feature = "lang-java")]
            Language::Java => Some(tree_sitter_java::LANGUAGE),
            #[cfg(feature = "lang-c")]
            Language::C => Some(tree_sitter_c::LANGUAGE),
            #[cfg(feature = "lang-cpp")]
            Language::Cpp => Some(tree_sitter_cpp::LANGUAGE),
            #[cfg(feature = "lang-bash")]
            Language::Bash => Some(tree_sitter_bash::LANGUAGE),
            #[cfg(feature = "lang-lua")]
            Language::Lua => Some(tree_sitter_lua::LANGUAGE),
            #[cfg(feature = "lang-css")]
            Language::Css => Some(tree_sitter_css::LANGUAGE),
            #[cfg(feature = "lang-hcl")]
            Language::Hcl => Some(tree_sitter_hcl::LANGUAGE),
            #[cfg(feature = "lang-markdown")]
            Language::Markdown => Some(tree_sitter_md::LANGUAGE),
            #[cfg(feature = "lang-sql")]
            Language::Sql => Some(tree_sitter_sequel::LANGUAGE),
            #[cfg(feature = "lang-toml")]
            Language::Toml => Some(tree_sitter_toml_ng::LANGUAGE),
            #[cfg(feature = "lang-json")]
            Language::Json => Some(tree_sitter_json::LANGUAGE),
            #[cfg(feature = "lang-yaml")]
            Language::Yaml => Some(tree_sitter_yaml::LANGUAGE),
            _ => None,
        }
    }

    /// Scope taxonomy for this language.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn tree_sitter_config(self) -> TreeSitterConfig {
        match self {
            Language::Rust => TreeSitterConfig {
                structure_kinds: &[
                    "function_item",
                    "impl_item",
                    "struct_item",
                    "enum_item",
                    "trait_item",
                    "mod_item",
                ],
                promoted_kinds: &[],
                function_body_kinds: &["function_item", "closure_expression"],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[("impl_item", NameRule::ImplHeader)],
                definition_kinds: &[
                    ("function_item", DefinitionKind::Function),
                    ("impl_item", DefinitionKind::Impl),
                    ("struct_item", DefinitionKind::Struct),
                    ("enum_item", DefinitionKind::Enum),
                    ("trait_item", DefinitionKind::Trait),
                    ("mod_item", DefinitionKind::Module),
                ],
                leaf_definitions: RUST_LEAVES,
                body_kinds: &["macro_rule"],
                import_kinds: &["use_declaration", "extern_crate_declaration"],
                leading_comment_kinds: &["line_comment", "block_comment", "attribute_item"],
                transparent_expansion: true,
                visibility: VisibilityRule::Rust,
            },
            Language::Python => TreeSitterConfig {
                structure_kinds: &["function_definition", "class_definition"],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition", "lambda"],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[("assignment", NameRule::Field("left"))],
                definition_kinds: &[
                    ("function_definition", DefinitionKind::Function),
                    ("class_definition", DefinitionKind::Class),
                ],
                leaf_definitions: PYTHON_LEAVES,
                body_kinds: &[],
                import_kinds: &[
                    "import_statement",
                    "import_from_statement",
                    "future_import_statement",
                ],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::Underscore,
            },
            Language::JavaScript => TreeSitterConfig {
                structure_kinds: &[
                    "function_declaration",
                    "class_declaration",
                    "method_definition",
                ],
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
                // Anonymous callbacks (`xs.map(x => …)`, `it("…", () =>
                // {…})`, `app.get("/…", (req, res) => {…})`) are the
                // relevant unit of behaviour but have no syntactic name.
                // Treat them as hunk anchors; the call-site label on the
                // opening line speaks for itself.
                anchor_only_kinds: &[
                    "arrow_function",
                    "function_expression",
                    "function",
                    "generator_function",
                ],
                // Labeled callbacks (`describe("…", () => {})`,
                // `it("…", () => {})`, `app.get("/…", () => {})`) carry
                // their identity on the call site, not the inner arrow.
                // Promote the call to a named structure so it appears in
                // the breadcrumb with a real name (callee + first
                // string-literal arg).
                call_promoted_kinds: &["call_expression"],
                name_rules: &[],
                definition_kinds: &[
                    ("function_declaration", DefinitionKind::Function),
                    ("class_declaration", DefinitionKind::Class),
                    ("method_definition", DefinitionKind::Method),
                    ("field_definition", DefinitionKind::Method),
                    ("variable_declarator", DefinitionKind::Function),
                    ("call_expression", DefinitionKind::Test),
                ],
                leaf_definitions: JAVASCRIPT_LEAVES,
                body_kinds: &[],
                import_kinds: &["import_statement"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::Exported,
            },
            Language::TypeScript => TreeSitterConfig {
                structure_kinds: &[
                    "function_declaration",
                    "class_declaration",
                    "method_definition",
                    "interface_declaration",
                    "type_alias_declaration",
                ],
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
                anchor_only_kinds: &[
                    "arrow_function",
                    "function_expression",
                    "function",
                    "generator_function",
                ],
                call_promoted_kinds: &["call_expression"],
                name_rules: &[],
                definition_kinds: &[
                    ("function_declaration", DefinitionKind::Function),
                    ("class_declaration", DefinitionKind::Class),
                    ("method_definition", DefinitionKind::Method),
                    ("interface_declaration", DefinitionKind::Interface),
                    ("type_alias_declaration", DefinitionKind::TypeAlias),
                    ("public_field_definition", DefinitionKind::Method),
                    ("variable_declarator", DefinitionKind::Function),
                    ("call_expression", DefinitionKind::Test),
                ],
                leaf_definitions: TYPESCRIPT_LEAVES,
                body_kinds: &[],
                import_kinds: &["import_statement"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::Exported,
            },
            Language::Tsx => TreeSitterConfig {
                structure_kinds: &[
                    "function_declaration",
                    "class_declaration",
                    "method_definition",
                    "interface_declaration",
                    "type_alias_declaration",
                ],
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
                anchor_only_kinds: &[
                    "arrow_function",
                    "function_expression",
                    "function",
                    "generator_function",
                ],
                call_promoted_kinds: &["call_expression"],
                name_rules: &[],
                definition_kinds: &[
                    ("function_declaration", DefinitionKind::Function),
                    ("class_declaration", DefinitionKind::Class),
                    ("method_definition", DefinitionKind::Method),
                    ("interface_declaration", DefinitionKind::Interface),
                    ("type_alias_declaration", DefinitionKind::TypeAlias),
                    ("public_field_definition", DefinitionKind::Method),
                    ("variable_declarator", DefinitionKind::Function),
                    ("call_expression", DefinitionKind::Test),
                ],
                leaf_definitions: TYPESCRIPT_LEAVES,
                body_kinds: &[],
                import_kinds: &["import_statement"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::Exported,
            },
            Language::Go => TreeSitterConfig {
                structure_kinds: &[
                    "function_declaration",
                    "method_declaration",
                    "type_declaration",
                ],
                promoted_kinds: &[],
                function_body_kinds: &[
                    "function_declaration",
                    "method_declaration",
                    "func_literal",
                ],
                // Anonymous Go function literals (`func(…) { … }`)
                // anchor labeled callbacks like `t.Run("…", func(…) {})`
                // and `http.HandleFunc("/…", func(w, r) {})`. Their
                // identity sits on the surrounding call.
                anchor_only_kinds: &["func_literal"],
                call_promoted_kinds: &["call_expression"],
                name_rules: &[
                    ("method_declaration", NameRule::Receiver),
                    ("type_declaration", NameRule::Specs),
                ],
                definition_kinds: &[
                    ("function_declaration", DefinitionKind::Function),
                    ("method_declaration", DefinitionKind::Method),
                    ("type_declaration", DefinitionKind::Type),
                    ("call_expression", DefinitionKind::Test),
                ],
                leaf_definitions: GO_LEAVES,
                body_kinds: &[],
                import_kinds: &["import_declaration"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::Capitalized,
            },
            Language::Ruby => TreeSitterConfig {
                structure_kinds: &["method", "singleton_method", "class", "module"],
                promoted_kinds: &[],
                function_body_kinds: &["method", "singleton_method", "block", "do_block", "lambda"],
                // Ruby blocks (`do … end`, `{ … }`) and lambdas are
                // anonymous; their identity sits on the surrounding
                // labeled call (RSpec `it "…" do … end`, Sinatra
                // `get "/…" do … end`, Rails `routes.draw do … end`).
                anchor_only_kinds: &["block", "do_block", "lambda"],
                call_promoted_kinds: &["call"],
                name_rules: &[
                    ("singleton_method", NameRule::SingletonMethod),
                    ("assignment", NameRule::Field("left")),
                ],
                definition_kinds: &[
                    ("method", DefinitionKind::Method),
                    ("singleton_method", DefinitionKind::Method),
                    ("class", DefinitionKind::Class),
                    ("module", DefinitionKind::Module),
                    ("call", DefinitionKind::Test),
                ],
                leaf_definitions: RUBY_LEAVES,
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Kotlin => TreeSitterConfig {
                structure_kinds: &[
                    "class_declaration",
                    "function_declaration",
                    "object_declaration",
                ],
                promoted_kinds: &[],
                function_body_kinds: &["function_declaration", "lambda_literal"],
                anchor_only_kinds: &["lambda_literal"],
                call_promoted_kinds: &[],
                name_rules: &[("property_declaration", NameRule::Property)],
                definition_kinds: &[
                    ("class_declaration", DefinitionKind::Class),
                    ("function_declaration", DefinitionKind::Function),
                    ("object_declaration", DefinitionKind::Object),
                ],
                leaf_definitions: KOTLIN_LEAVES,
                body_kinds: &["class_body", "function_body", "enum_class_body"],
                import_kinds: &["import", "import_header", "import_list"],
                leading_comment_kinds: &["line_comment", "block_comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Java => TreeSitterConfig {
                structure_kinds: &[
                    "class_declaration",
                    "interface_declaration",
                    "method_declaration",
                    "constructor_declaration",
                ],
                promoted_kinds: &[],
                function_body_kinds: &[
                    "method_declaration",
                    "constructor_declaration",
                    "lambda_expression",
                ],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[("field_declaration", NameRule::Declarator)],
                definition_kinds: &[
                    ("class_declaration", DefinitionKind::Class),
                    ("interface_declaration", DefinitionKind::Interface),
                    ("method_declaration", DefinitionKind::Method),
                    ("constructor_declaration", DefinitionKind::Constructor),
                ],
                leaf_definitions: JAVA_LEAVES,
                body_kinds: &[],
                import_kinds: &["import_declaration"],
                leading_comment_kinds: &["line_comment", "block_comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::C => TreeSitterConfig {
                structure_kinds: &["function_definition", "struct_specifier"],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition"],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("function_definition", NameRule::Declarator),
                    ("declaration", NameRule::Declarator),
                    ("type_definition", NameRule::Declarator),
                ],
                definition_kinds: &[
                    ("function_definition", DefinitionKind::Function),
                    ("struct_specifier", DefinitionKind::Struct),
                ],
                leaf_definitions: C_LEAVES,
                body_kinds: &[],
                import_kinds: &["preproc_include"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Cpp => TreeSitterConfig {
                structure_kinds: &[
                    "function_definition",
                    "class_specifier",
                    "namespace_definition",
                ],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition", "lambda_expression"],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("function_definition", NameRule::Declarator),
                    ("declaration", NameRule::Declarator),
                    ("field_declaration", NameRule::Declarator),
                    ("type_definition", NameRule::Declarator),
                ],
                definition_kinds: &[
                    ("function_definition", DefinitionKind::Function),
                    ("class_specifier", DefinitionKind::Class),
                    ("namespace_definition", DefinitionKind::Namespace),
                ],
                leaf_definitions: CPP_LEAVES,
                body_kinds: &[],
                import_kinds: &["preproc_include", "using_declaration"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Bash => TreeSitterConfig {
                structure_kinds: &["function_definition"],
                promoted_kinds: &[],
                function_body_kinds: &["function_definition"],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[],
                definition_kinds: &[("function_definition", DefinitionKind::Function)],
                leaf_definitions: &[],
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Lua => TreeSitterConfig {
                structure_kinds: &["function_declaration"],
                promoted_kinds: &[],
                function_body_kinds: &["function_declaration", "function_definition"],
                // Anonymous Lua functions (`function() … end`) are
                // anchors but have no syntactic name; their identity
                // sits on the surrounding labeled call (busted's
                // `describe(“…”, function() … end)`, Neovim
                // `vim.keymap.set(“…”, function() … end)`).
                anchor_only_kinds: &["function_definition"],
                call_promoted_kinds: &["function_call"],
                name_rules: &[("assignment_statement", NameRule::ChildKind("variable_list"))],
                definition_kinds: &[
                    ("function_declaration", DefinitionKind::Function),
                    ("function_call", DefinitionKind::Test),
                ],
                leaf_definitions: LUA_LEAVES,
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Css => TreeSitterConfig {
                structure_kinds: &["rule_set", "media_statement"],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("rule_set", NameRule::Header),
                    ("media_statement", NameRule::Header),
                    ("keyframes_statement", NameRule::Header),
                    ("supports_statement", NameRule::Header),
                ],
                definition_kinds: &[
                    ("rule_set", DefinitionKind::Rule),
                    ("media_statement", DefinitionKind::Block),
                ],
                leaf_definitions: CSS_LEAVES,
                body_kinds: &["block", "keyframe_block_list"],
                import_kinds: &["import_statement"],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Hcl => TreeSitterConfig {
                structure_kinds: &["block"],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("block", NameRule::Positional),
                    ("attribute", NameRule::ChildKind("identifier")),
                ],
                definition_kinds: &[("block", DefinitionKind::Block)],
                leaf_definitions: HCL_LEAVES,
                body_kinds: &["body", "expression"],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Markdown => TreeSitterConfig {
                structure_kinds: &["atx_heading", "setext_heading"],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("atx_heading", NameRule::HeadingText),
                    ("setext_heading", NameRule::HeadingText),
                    ("section", NameRule::SectionHeading),
                ],
                definition_kinds: &[],
                leaf_definitions: MARKDOWN_LEAVES,
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &[],
                // Prose: body changes have no structure ancestor, and the
                // outermost transparent ancestor is a whole heading-
                // delimited section (often the entire document). Disable
                // transparent expansion so such changes use 3-line default
                // context instead of swallowing the file. A dedicated
                // section model is future work.
                transparent_expansion: false,
                visibility: VisibilityRule::None,
            },
            Language::Sql => TreeSitterConfig {
                // DDL statements are the named units of a schema. Each
                // parses as `statement > create_*/alter_*`; the wrapper
                // `statement` stays transparent so the breadcrumb reads
                // e.g. `[create_table users]`.
                structure_kinds: &[
                    "create_table",
                    "create_view",
                    "create_materialized_view",
                    "create_function",
                    "create_index",
                    "create_trigger",
                    "create_type",
                    "create_schema",
                    "create_sequence",
                    "alter_table",
                ],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("create_table", NameRule::ObjectReference),
                    ("create_view", NameRule::ObjectReference),
                    ("create_materialized_view", NameRule::ObjectReference),
                    ("create_function", NameRule::ObjectReference),
                    ("create_index", NameRule::ObjectReference),
                    ("create_trigger", NameRule::ObjectReference),
                    ("create_type", NameRule::ObjectReference),
                    ("create_schema", NameRule::ObjectReference),
                    ("create_sequence", NameRule::ObjectReference),
                    ("alter_table", NameRule::ObjectReference),
                ],
                definition_kinds: &[
                    ("create_table", DefinitionKind::Statement),
                    ("create_view", DefinitionKind::Statement),
                    ("create_materialized_view", DefinitionKind::Statement),
                    ("create_function", DefinitionKind::Statement),
                    ("create_index", DefinitionKind::Statement),
                    ("create_trigger", DefinitionKind::Statement),
                    ("create_type", DefinitionKind::Statement),
                    ("create_schema", DefinitionKind::Statement),
                    ("create_sequence", DefinitionKind::Statement),
                    ("alter_table", DefinitionKind::Statement),
                ],
                leaf_definitions: &[],
                body_kinds: &["column_definitions"],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Toml => TreeSitterConfig {
                structure_kinds: &["table", "table_array_element"],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[
                    ("table", NameRule::TableKey),
                    ("table_array_element", NameRule::TableKey),
                    ("pair", NameRule::TableKey),
                ],
                definition_kinds: &[
                    ("table", DefinitionKind::Table),
                    ("table_array_element", DefinitionKind::Table),
                ],
                leaf_definitions: TOML_LEAVES,
                body_kinds: &[
                    "pair",
                    "string",
                    "integer",
                    "float",
                    "boolean",
                    "offset_date_time",
                    "local_date_time",
                    "local_date",
                    "local_time",
                    "array",
                    "inline_table",
                ],
                import_kinds: &[],
                leading_comment_kinds: &["comment"],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Json => TreeSitterConfig {
                structure_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[("pair", NameRule::QuotedKey)],
                definition_kinds: &[],
                leaf_definitions: JSON_LEAVES,
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &[],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
            },
            Language::Yaml => TreeSitterConfig {
                structure_kinds: &[],
                promoted_kinds: &[],
                function_body_kinds: &[],
                anchor_only_kinds: &[],
                call_promoted_kinds: &[],
                name_rules: &[("block_mapping_pair", NameRule::QuotedKey)],
                definition_kinds: &[],
                leaf_definitions: YAML_LEAVES,
                body_kinds: &[],
                import_kinds: &[],
                leading_comment_kinds: &[],
                transparent_expansion: true,
                visibility: VisibilityRule::None,
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
            "Kotlin" => Some(Language::Kotlin),
            "Lua" => Some(Language::Lua),
            "Markdown" => Some(Language::Markdown),
            "Python" => Some(Language::Python),
            "Ruby" => Some(Language::Ruby),
            "Rust" => Some(Language::Rust),
            "SQL" => Some(Language::Sql),
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
    DETECTION_SYNTAX_SET.get_or_init(crate::config::bundled_syntax_set)
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
    fn detects_kotlin_sources_and_scripts() {
        for path in ["Greeter.kt", "build.gradle.kts"] {
            assert_eq!(
                Language::detect(path, "fun greet() {}\n"),
                Some(Language::Kotlin),
                "{path}"
            );
            assert_eq!(
                Language::detect_highlight_name(path, "fun greet() {}\n").as_deref(),
                Some("Kotlin"),
                "{path}"
            );
        }
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
            Language::Kotlin,
            Language::Lua,
            Language::Markdown,
            Language::Python,
            Language::Ruby,
            Language::Rust,
            Language::Sql,
            Language::Toml,
            Language::Tsx,
            Language::TypeScript,
            Language::Yaml,
        ] {
            assert_eq!(Language::from_id(language.id()), Some(language));
        }
    }
}
