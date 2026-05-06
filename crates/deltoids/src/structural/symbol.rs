//! Symbols: language-agnostic representation of named declarations.
//!
//! A [`Symbol`] is one declaration we care about for structural diffing:
//! a function, method, class, struct, enum, trait, type alias, constant,
//! interface, etc. Symbols carry enough information for the pairing /
//! classification stages in [`crate::structural`] to work without
//! re-walking the AST.
//!
//! The extractor [`extract_symbols`] is the only public entry. It is a
//! deep interface: callers pass a path + source string and receive a
//! flat `Vec<Symbol>` describing the file. Internally it dispatches per
//! language to a tree-sitter visitor.

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::Language;
use crate::syntax::ParsedFile;

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// What kind of declaration a symbol is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Free function (`fn` in Rust, `def` in Python at module level, etc.).
    Function,
    /// Method (function attached to a type/class/impl).
    Method,
    /// Class (Python `class`, Java `class`, TS `class`, …).
    Class,
    /// Rust `struct`, C/C++ `struct`, Go type-with-struct.
    Struct,
    /// Enumerated type (`enum` in Rust/Java/TS, Go enum-equivalent).
    Enum,
    /// Trait (Rust) / interface (Java/TS).
    Trait,
    /// Type alias / typedef.
    Type,
    /// Top-level constant or `static`.
    Const,
    /// Module / namespace / package.
    Module,
    /// Field of a struct/class. Only emitted where the schema-level view
    /// matters; skipped for languages without a stable concept.
    Field,
    /// Macro / decorator definition.
    Macro,
    /// Implementation block (Rust `impl Foo`).
    Impl,
    /// Anything we recognize as a named container but doesn't fit one of
    /// the above. Carries the tree-sitter node kind for diagnostics.
    Other(String),
}

/// Visibility of a declaration. We're conservative: when unsure, mark
/// `Public` so changes don't get hidden in public-only views.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
    Crate,
    /// Restricted to a specific module / package path.
    Restricted(String),
}

/// 1-indexed inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

impl LineSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn contains(&self, line: usize) -> bool {
        line >= self.start && line <= self.end
    }
}

/// Qualified name path (`["Foo", "bar"]` for `Foo::bar` or `Foo.bar`).
pub type SymbolPath = Vec<String>;

/// A named declaration extracted from a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub path: SymbolPath,
    pub visibility: Visibility,
    /// Declaration line(s) without the body. Used for change descriptions
    /// and signature-only views. Trailing whitespace stripped, no opening
    /// brace.
    pub signature: String,
    /// Where this symbol's declaration starts and ends (1-indexed
    /// inclusive). Includes any body and closing brace.
    pub span: LineSpan,
    /// Where the body interior of this symbol lives, if it has one.
    /// Excludes the opening / closing delimiters.
    pub body_span: Option<LineSpan>,
    /// Raw text of the body, when present. Used by the classifier to
    /// detect body-only changes without being fooled by span shifts
    /// (symbols can move lines without their content changing).
    pub body_text: Option<String>,
    pub language: Language,
}

impl Symbol {
    /// Display path joined with `::`. Stable across languages.
    pub fn qualified_name(&self) -> String {
        self.path.join("::")
    }

    /// Signature with leading visibility / accessibility tokens
    /// removed, so two symbols that differ only in visibility have the
    /// same `core_signature`. Used by the classifier to separate
    /// pure-visibility changes from signature changes.
    pub fn core_signature(&self) -> String {
        let mut s = self.signature.trim_start();
        // Per-language strip of leading visibility tokens.
        let prefixes: &[&str] = match self.language {
            Language::Rust => &["pub(crate)", "pub(super)", "pub(self)", "pub"],
            Language::Java | Language::TypeScript | Language::Tsx | Language::JavaScript => {
                &["public", "private", "protected"]
            }
            _ => &[],
        };
        for prefix in prefixes {
            if let Some(rest) = strip_complete_token(s, prefix) {
                s = rest;
                break;
            }
        }
        // Also drop a `pub(in something)` form.
        if let Some(rest) = s.strip_prefix("pub(in ")
            && let Some(end) = rest.find(')')
        {
            s = rest[end + 1..].trim_start();
        }
        s.to_string()
    }
}

/// Strip `prefix` from `s` only if it's followed by whitespace (i.e.
/// a complete token). Avoids munging things like `public_foo`.
fn strip_complete_token<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?;
    let trimmed = rest.trim_start();
    if rest.len() == trimmed.len() {
        return None;
    }
    Some(trimmed)
}

// ---------------------------------------------------------------------------
// Public extraction entry
// ---------------------------------------------------------------------------

/// Extract every named declaration from `source` for the language
/// detected from `path`. Returns an empty vec for unsupported languages
/// or parse failures.
///
/// Symbols are returned in source order (depth-first preorder).
pub fn extract_symbols(path: &str, source: &str) -> Vec<Symbol> {
    let Some(parsed) = ParsedFile::parse(path, source) else {
        return Vec::new();
    };
    let Some(language) = Language::detect(path, source) else {
        return Vec::new();
    };
    extract_symbols_from(&parsed, language, source)
}

pub(crate) fn extract_symbols_from(
    parsed: &ParsedFile,
    language: Language,
    source: &str,
) -> Vec<Symbol> {
    let root = parsed.root_node();
    let bytes = parsed.source_bytes();
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk(root, source, bytes, language, &mut path, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Generic walker
// ---------------------------------------------------------------------------

/// Walk `node` and append any symbols it (or its descendants) defines to
/// `out`. `path` is the qualified-name prefix for nested children.
fn walk<'a>(
    node: Node<'a>,
    source: &'a str,
    bytes: &'a [u8],
    language: Language,
    path: &mut SymbolPath,
    out: &mut Vec<Symbol>,
) {
    if let Some(action) = classify_node(node, language, bytes) {
        match action {
            NodeAction::Emit(symbol_action) => {
                let SymbolAction {
                    kind,
                    name,
                    body,
                    visibility,
                    pushes_path,
                } = symbol_action;
                let span = node_line_span(node);
                let body_span = body.and_then(|b| body_interior_span(b, bytes));
                let body_text = body.and_then(|b| body_interior_text(b, bytes));
                let signature = compute_signature(node, body, source);
                let mut full_path = path.clone();
                full_path.push(name.clone());
                out.push(Symbol {
                    kind,
                    path: full_path,
                    visibility,
                    signature,
                    span,
                    body_span,
                    body_text,
                    language,
                });
                if pushes_path {
                    path.push(name);
                    walk_children(node, source, bytes, language, path, out);
                    path.pop();
                    return;
                }
            }
            NodeAction::PushPathOnly(name) => {
                // E.g. Rust `impl Foo` doesn't emit a symbol but adds
                // `Foo` to the qualified-name prefix for nested methods.
                path.push(name);
                walk_children(node, source, bytes, language, path, out);
                path.pop();
                return;
            }
        }
    }

    walk_children(node, source, bytes, language, path, out);
}

fn walk_children<'a>(
    node: Node<'a>,
    source: &'a str,
    bytes: &'a [u8],
    language: Language,
    path: &mut SymbolPath,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk(cursor.node(), source, bytes, language, path, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-node classification
// ---------------------------------------------------------------------------

enum NodeAction<'a> {
    /// Emit a symbol for this node.
    Emit(SymbolAction<'a>),
    /// Don't emit, but push a name onto the path prefix and walk
    /// children (e.g. Rust `impl Foo` — its inner methods get the
    /// qualified prefix `Foo::`).
    PushPathOnly(String),
}

struct SymbolAction<'a> {
    kind: SymbolKind,
    name: String,
    body: Option<Node<'a>>,
    visibility: Visibility,
    /// True when this symbol's name should be added to the qualified
    /// prefix when walking its body. Classes / structs / traits do this;
    /// functions don't.
    pushes_path: bool,
}

fn classify_node<'a>(node: Node<'a>, language: Language, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match language {
        Language::Rust => classify_rust(node, bytes),
        Language::Python => classify_python(node, bytes),
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            classify_typescript(node, bytes)
        }
        Language::Go => classify_go(node, bytes),
        Language::Java => classify_java(node, bytes),
        Language::C | Language::Cpp => classify_c_cpp(node, bytes),
        Language::Ruby => classify_ruby(node, bytes),
        // Languages that don't have classical "named declarations" yet:
        // markdown, json, yaml, toml, css, hcl, lua, bash. We'll add
        // when the use case arises.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

fn classify_rust<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    let kind = node.kind();
    match kind {
        "function_item" | "function_signature_item" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            let visibility = rust_visibility(node, bytes);
            // Method when the enclosing declaration_list belongs to an
            // impl_item / trait_item.
            let kind = if rust_is_method(node) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            Some(NodeAction::Emit(SymbolAction {
                kind,
                name,
                body,
                visibility,
                pushes_path: false,
            }))
        }
        "struct_item" => simple_emit(node, bytes, SymbolKind::Struct, rust_visibility, true),
        "enum_item" => simple_emit(node, bytes, SymbolKind::Enum, rust_visibility, true),
        "union_item" => simple_emit(node, bytes, SymbolKind::Struct, rust_visibility, true),
        "trait_item" => simple_emit(node, bytes, SymbolKind::Trait, rust_visibility, true),
        "type_item" => simple_emit(node, bytes, SymbolKind::Type, rust_visibility, false),
        "const_item" => simple_emit(node, bytes, SymbolKind::Const, rust_visibility, false),
        "static_item" => simple_emit(node, bytes, SymbolKind::Const, rust_visibility, false),
        "macro_definition" => simple_emit(node, bytes, SymbolKind::Macro, rust_visibility, false),
        "mod_item" => simple_emit(node, bytes, SymbolKind::Module, rust_visibility, true),
        "impl_item" => {
            // `impl Foo` and `impl Trait for Foo`: push the implementor
            // type onto the path so methods become `Foo::method`. We
            // ignore the `trait` field for the path; otherwise method
            // names like `Drawable::Foo::draw` would not match anywhere.
            let type_name = field_text(node, "type", bytes)?;
            Some(NodeAction::PushPathOnly(type_name))
        }
        _ => None,
    }
}

fn simple_emit<'a, F>(
    node: Node<'a>,
    bytes: &[u8],
    kind: SymbolKind,
    visibility: F,
    pushes_path: bool,
) -> Option<NodeAction<'a>>
where
    F: FnOnce(Node<'a>, &[u8]) -> Visibility,
{
    let name = field_text(node, "name", bytes)?;
    let body = node.child_by_field_name("body");
    Some(NodeAction::Emit(SymbolAction {
        kind,
        name,
        body,
        visibility: visibility(node, bytes),
        pushes_path,
    }))
}

fn rust_visibility(node: Node<'_>, bytes: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let txt = child.utf8_text(bytes).unwrap_or("");
            return parse_rust_visibility(txt);
        }
    }
    Visibility::Private
}

fn parse_rust_visibility(text: &str) -> Visibility {
    let trimmed = text.trim();
    if trimmed == "pub" {
        Visibility::Public
    } else if trimmed == "pub(crate)" {
        Visibility::Crate
    } else if let Some(rest) = trimmed
        .strip_prefix("pub(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let rest = rest.trim();
        let rest = rest.strip_prefix("in ").unwrap_or(rest);
        Visibility::Restricted(rest.to_string())
    } else {
        Visibility::Public
    }
}

fn rust_is_method(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "declaration_list" {
        return false;
    }
    let Some(grand) = parent.parent() else {
        return false;
    };
    matches!(grand.kind(), "impl_item" | "trait_item")
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn classify_python<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "function_definition" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            let kind = if python_is_method(node) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            Some(NodeAction::Emit(SymbolAction {
                kind,
                name: name.clone(),
                body,
                visibility: python_visibility(&name),
                pushes_path: false,
            }))
        }
        "class_definition" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Class,
                name: name.clone(),
                body,
                visibility: python_visibility(&name),
                pushes_path: true,
            }))
        }
        _ => None,
    }
}

fn python_is_method(node: Node<'_>) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "class_definition" {
            return true;
        }
        if p.kind() == "function_definition" {
            // Nested in another function — local helper, not a method.
            return false;
        }
        cur = p.parent();
    }
    false
}

fn python_visibility(name: &str) -> Visibility {
    if name.starts_with('_') && !(name.starts_with("__") && name.ends_with("__")) {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript
// ---------------------------------------------------------------------------

fn classify_typescript<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Function,
                name,
                body,
                visibility: ts_top_level_visibility(node),
                pushes_path: false,
            }))
        }
        "class_declaration" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Class,
                name,
                body,
                visibility: ts_top_level_visibility(node),
                pushes_path: true,
            }))
        }
        "interface_declaration" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Trait,
                name,
                body,
                visibility: ts_top_level_visibility(node),
                pushes_path: true,
            }))
        }
        "enum_declaration" => simple_emit(
            node,
            bytes,
            SymbolKind::Enum,
            |n, _| ts_top_level_visibility(n),
            true,
        ),
        "type_alias_declaration" => simple_emit(
            node,
            bytes,
            SymbolKind::Type,
            |n, _| ts_top_level_visibility(n),
            false,
        ),
        "method_definition" | "method_signature" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Method,
                name,
                body,
                visibility: ts_method_visibility(node, bytes),
                pushes_path: false,
            }))
        }
        _ => None,
    }
}

fn ts_top_level_visibility(node: Node<'_>) -> Visibility {
    let Some(parent) = node.parent() else {
        return Visibility::Private;
    };
    if parent.kind() == "export_statement" {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

fn ts_method_visibility(node: Node<'_>, bytes: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "accessibility_modifier" {
            let txt = child.utf8_text(bytes).unwrap_or("").trim();
            return match txt {
                "public" => Visibility::Public,
                "private" => Visibility::Private,
                "protected" => Visibility::Restricted("protected".to_string()),
                _ => Visibility::Public,
            };
        }
    }
    // No modifier means public in TS.
    Visibility::Public
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn classify_go<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "function_declaration" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Function,
                name: name.clone(),
                body,
                visibility: go_visibility(&name),
                pushes_path: false,
            }))
        }
        "method_declaration" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Method,
                name: name.clone(),
                body,
                visibility: go_visibility(&name),
                pushes_path: false,
            }))
        }
        "type_declaration" => {
            // Could be struct, interface, or alias — peek inside.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_spec" {
                    let name = field_text(child, "name", bytes)?;
                    let kind = match child.child_by_field_name("type").map(|n| n.kind()) {
                        Some("struct_type") => SymbolKind::Struct,
                        Some("interface_type") => SymbolKind::Trait,
                        _ => SymbolKind::Type,
                    };
                    return Some(NodeAction::Emit(SymbolAction {
                        kind,
                        name: name.clone(),
                        body: None,
                        visibility: go_visibility(&name),
                        pushes_path: false,
                    }));
                }
            }
            None
        }
        _ => None,
    }
}

fn go_visibility(name: &str) -> Visibility {
    match name.chars().next() {
        Some(c) if c.is_uppercase() => Visibility::Public,
        _ => Visibility::Private,
    }
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

fn classify_java<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "class_declaration" => simple_emit(node, bytes, SymbolKind::Class, java_visibility, true),
        "interface_declaration" => {
            simple_emit(node, bytes, SymbolKind::Trait, java_visibility, true)
        }
        "enum_declaration" => simple_emit(node, bytes, SymbolKind::Enum, java_visibility, true),
        "method_declaration" | "constructor_declaration" => {
            simple_emit(node, bytes, SymbolKind::Method, java_visibility, false)
        }
        _ => None,
    }
}

fn java_visibility(node: Node<'_>, bytes: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let txt = child.utf8_text(bytes).unwrap_or("");
            if txt.contains("public") {
                return Visibility::Public;
            } else if txt.contains("private") {
                return Visibility::Private;
            } else if txt.contains("protected") {
                return Visibility::Restricted("protected".to_string());
            }
        }
    }
    // No modifier = package-private.
    Visibility::Restricted("package".to_string())
}

// ---------------------------------------------------------------------------
// C / C++
// ---------------------------------------------------------------------------

fn classify_c_cpp<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "function_definition" => {
            // Function name lives under the declarator. Walk down to
            // find an identifier.
            let declarator = node.child_by_field_name("declarator")?;
            let name = c_function_name(declarator, bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Function,
                name,
                body,
                visibility: Visibility::Public,
                pushes_path: false,
            }))
        }
        "struct_specifier" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Struct,
                name,
                body,
                visibility: Visibility::Public,
                pushes_path: true,
            }))
        }
        "class_specifier" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Class,
                name,
                body,
                visibility: Visibility::Public,
                pushes_path: true,
            }))
        }
        _ => None,
    }
}

fn c_function_name(declarator: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cur = declarator;
    loop {
        if cur.kind() == "identifier" || cur.kind() == "field_identifier" {
            return cur.utf8_text(bytes).ok().map(|s| s.to_string());
        }
        let next = cur.child_by_field_name("declarator")?;
        if next.id() == cur.id() {
            return None;
        }
        cur = next;
    }
}

// ---------------------------------------------------------------------------
// Ruby
// ---------------------------------------------------------------------------

fn classify_ruby<'a>(node: Node<'a>, bytes: &[u8]) -> Option<NodeAction<'a>> {
    match node.kind() {
        "method" | "singleton_method" => {
            let name = field_text(node, "name", bytes)?;
            let body = node.child_by_field_name("body");
            Some(NodeAction::Emit(SymbolAction {
                kind: SymbolKind::Method,
                name,
                body,
                visibility: Visibility::Public,
                pushes_path: false,
            }))
        }
        "class" => simple_emit(
            node,
            bytes,
            SymbolKind::Class,
            |_, _| Visibility::Public,
            true,
        ),
        "module" => simple_emit(
            node,
            bytes,
            SymbolKind::Module,
            |_, _| Visibility::Public,
            true,
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn field_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    let n = node.child_by_field_name(field)?;
    n.utf8_text(bytes).ok().map(|s| s.to_string())
}

fn node_line_span(node: Node<'_>) -> LineSpan {
    LineSpan::new(node.start_position().row + 1, node.end_position().row + 1)
}

/// Compute the signature of a declaration: source from the start of
/// `node` up to (but excluding) the body, with trailing whitespace and
/// any opening `{` stripped. If the node has no body, take the full
/// node text minus a trailing `;` if present.
fn compute_signature(node: Node<'_>, body: Option<Node<'_>>, source: &str) -> String {
    let start = node.start_byte();
    let end = body.map(|b| b.start_byte()).unwrap_or(node.end_byte());
    let raw = source.get(start..end).unwrap_or("");
    let trimmed = raw.trim_end();
    let trimmed = trimmed
        .strip_suffix('{')
        .map(|s| s.trim_end())
        .unwrap_or(trimmed);
    let trimmed = trimmed
        .strip_suffix(';')
        .map(|s| s.trim_end())
        .unwrap_or(trimmed);
    trimmed.to_string()
}

/// Compute the body interior span: the lines covered by the body's
/// children excluding pure delimiter tokens (`{`, `}`, `(`, `)`).
fn body_interior_span(body: Node<'_>, bytes: &[u8]) -> Option<LineSpan> {
    let mut cursor = body.walk();
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    if cursor.goto_first_child() {
        loop {
            let n = cursor.node();
            if !is_delim_only(n, bytes) {
                let s = n.start_position().row + 1;
                let e = n.end_position().row + 1;
                start = Some(start.map(|x| x.min(s)).unwrap_or(s));
                end = Some(end.map(|x| x.max(e)).unwrap_or(e));
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    match (start, end) {
        (Some(s), Some(e)) if s <= e => Some(LineSpan::new(s, e)),
        _ => None,
    }
}

/// Concatenated text of the body's children that aren't delimiter
/// tokens. We use this for content equality checks; mirrors the
/// rule used by [`body_interior_span`] so the two stay aligned.
fn body_interior_text(body: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cursor = body.walk();
    let mut parts = Vec::new();
    if cursor.goto_first_child() {
        loop {
            let n = cursor.node();
            if !is_delim_only(n, bytes)
                && let Ok(t) = n.utf8_text(bytes)
            {
                parts.push(t.to_string());
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn is_delim_only(node: Node<'_>, bytes: &[u8]) -> bool {
    if node.named_child_count() > 0 {
        return false;
    }
    let txt = node.utf8_text(bytes).unwrap_or("");
    matches!(
        txt.trim(),
        "{" | "}" | "(" | ")" | "[" | "]" | "<" | ">" | ":" | ";" | ","
    )
}

#[cfg(test)]
mod tests;
