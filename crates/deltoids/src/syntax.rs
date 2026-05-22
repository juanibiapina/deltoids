//! Tree-sitter parsing and scope queries.
//!
//! Parses source code for the stable [`Language`] detected by the language
//! module. Constructed via [`ParsedFile::parse`]; all syntax-level questions
//! (enclosing scopes, structure-vs-data tier) are answered through methods on
//! [`ParsedFile`].

use tree_sitter::{Node, Parser, Point, Tree};

use crate::Language;
use crate::scope::ScopeNode;

/// A parsed source file: its syntax tree, the source it came from, and the
/// language taxonomy needed to interpret nodes. Constructed via
/// [`ParsedFile::parse`].
pub struct ParsedFile {
    tree: Tree,
    source: Vec<u8>,
    /// Node kinds used for the ancestor chain (display breadcrumb).
    structure_kinds: &'static [&'static str],
    /// Node kinds used as hunk anchors when no structure contains the change.
    data_kinds: &'static [&'static str],
    /// Wrapper kinds promoted to structure when their `value` field is a
    /// function body.
    promoted_kinds: &'static [&'static str],
    /// Node kinds that introduce a function body.
    function_body_kinds: &'static [&'static str],
    /// Anonymous function-body kinds that act as anchors but never appear
    /// in the breadcrumb. See [`crate::language::TreeSitterConfig`].
    anchor_only_kinds: &'static [&'static str],
    /// Wrapper kinds (e.g. `call_expression`) promoted to a named
    /// structure when one of their arguments is a function body. Their
    /// breadcrumb name is derived from the callee plus the first
    /// string-literal argument when present.
    call_promoted_kinds: &'static [&'static str],
    /// Node kinds whose breadcrumb name is built from concatenated
    /// positional `identifier` / `string_lit` children rather than a
    /// `name` field. See [`crate::language::TreeSitterConfig`].
    positional_name_kinds: &'static [&'static str],
    /// Node kinds that, when they appear as siblings above a structure,
    /// attach to that structure for scope queries. See
    /// [`crate::language::TreeSitterConfig`].
    leading_comment_kinds: &'static [&'static str],
}

impl ParsedFile {
    /// Parse `source` for the language detected from `path`.
    ///
    /// Returns `None` if the language is not recognized or parsing fails.
    pub fn parse(path: &str, source: &str) -> Option<Self> {
        let language = Language::detect(path, source)?;
        Self::parse_as(language, source)
    }

    pub(crate) fn parse_as(language: Language, source: &str) -> Option<Self> {
        let entry = language.tree_sitter_config();
        let mut parser = Parser::new();
        parser.set_language(&entry.language.into()).ok()?;
        let tree = parser.parse(source, None)?;
        Some(ParsedFile {
            tree,
            source: source.as_bytes().to_vec(),
            structure_kinds: entry.structure_kinds,
            data_kinds: entry.data_kinds,
            promoted_kinds: entry.promoted_kinds,
            function_body_kinds: entry.function_body_kinds,
            anchor_only_kinds: entry.anchor_only_kinds,
            call_promoted_kinds: entry.call_promoted_kinds,
            positional_name_kinds: entry.positional_name_kinds,
            leading_comment_kinds: entry.leading_comment_kinds,
        })
    }

    /// Return all enclosing scope nodes at the given 0-indexed `line`,
    /// outermost first. A scope is included when its tree-sitter kind is
    /// part of the language's structure or data tier, or is a wrapper kind
    /// that has been promoted because its value is a function body.
    /// Structures and promoted scopes nested inside another function body
    /// (local helpers) are excluded.
    pub fn enclosing_scopes(&self, line: usize) -> Vec<ScopeNode> {
        let point = self.point_at_first_non_whitespace(line);
        let Some(node) = self
            .tree
            .root_node()
            .descendant_for_point_range(point, point)
        else {
            return Vec::new();
        };

        // Method- and class-level decorators (e.g. `@Cron(...)` above a class
        // method or `@Injectable()` above a class) appear in the tree as
        // siblings of the decorated node, not as children. A query at a
        // decorator line would otherwise walk up through `class_body` straight
        // to the class, skipping the method the decorator belongs to.
        //
        // Doc comments (`///`, JSDoc, `# …` above a `def`, …) and Rust
        // attributes (`#[derive(…)]`) sit in the same shape: a sibling above
        // the structure they document, not a child. `skip_leading_comments`
        // walks past such siblings to the structure they precede so the
        // breadcrumb resolves to the documented item, not its parent.
        let node = skip_leading_comments(node, self.leading_comment_kinds);
        let node = skip_decorators(node);

        let mut ancestors = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            let kind_is_structure = self.structure_kinds.contains(&n.kind());
            let kind_is_data = self.data_kinds.contains(&n.kind());
            let kind_is_promoted = self.promoted_kinds.contains(&n.kind());
            let kind_is_anchor_only = self.anchor_only_kinds.contains(&n.kind());
            let kind_is_call_promoted = self.call_promoted_kinds.contains(&n.kind());
            let include = kind_is_structure
                || kind_is_data
                || kind_is_anchor_only
                || (kind_is_promoted && self.has_function_value(&n))
                || (kind_is_call_promoted && self.has_function_argument(n));
            // Demote structures, promoted, anchor-only, and call-promoted
            // scopes when they live inside another function body. Anchor-only
            // function bodies (`arrow_function`) don't demote: a labeled
            // callback inside another labeled callback (e.g. `it(…)` inside
            // `describe(…)`) is a sibling unit, not a local helper.
            // Data scopes (objects/arrays) are unaffected.
            let include = include
                && !((kind_is_structure
                    || kind_is_promoted
                    || kind_is_anchor_only
                    || kind_is_call_promoted)
                    && self.is_nested_in_function(n));
            if include {
                let start_line = decorator_adjusted_start(n) + 1;
                let end_line = node_end_line(n);
                let kind_is_positional = self.positional_name_kinds.contains(&n.kind());
                let name = self.scope_name(n, kind_is_call_promoted, kind_is_positional);
                let text = self
                    .source_line_raw(n.start_position().row)
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

    /// True when `scope`'s kind belongs to this language's structure tier.
    ///
    /// Promoted kinds (e.g. a JS class field whose value is an arrow
    /// function) also count as structures. `enclosing_scopes` only emits
    /// promoted-kind scopes when their value is function-like, so the
    /// kind alone is enough to decide.
    pub fn is_structure(&self, scope: &ScopeNode) -> bool {
        let kind = scope.kind.as_str();
        self.structure_kinds.contains(&kind)
            || self.promoted_kinds.contains(&kind)
            || self.anchor_only_kinds.contains(&kind)
            || self.call_promoted_kinds.contains(&kind)
    }

    /// True when `scope`'s kind is anchor-only — a hunk-anchor that must
    /// not appear in the breadcrumb. Anonymous callbacks (arrow functions,
    /// function expressions passed inline) carry their identity in the
    /// call signature on the opening line of the hunk, so a synthesised
    /// `[KIND name]` entry would just add noise.
    pub fn is_anchor_only(&self, scope: &ScopeNode) -> bool {
        self.anchor_only_kinds.contains(&scope.kind.as_str())
    }

    /// True when `scope`'s kind belongs to this language's data tier
    /// (anonymous containers like JS objects/arrays or YAML mappings).
    pub fn is_data(&self, scope: &ScopeNode) -> bool {
        self.data_kinds.contains(&scope.kind.as_str())
    }

    /// True when `node`'s `value` field holds a function body (arrow
    /// function, function expression, named function, or any other kind
    /// listed in `function_body_kinds`). Gates promotion of wrapper kinds
    /// (class fields, variable declarators) to structures.
    fn has_function_value(&self, node: &Node) -> bool {
        let Some(value) = node.child_by_field_name("value") else {
            return false;
        };
        self.function_body_kinds.contains(&value.kind())
    }

    /// Resolve the breadcrumb name for a scope node. Call-promoted nodes
    /// (e.g. `call_expression` in JS/TS) build their name from the callee
    /// plus the first string-literal argument. Positional-name nodes
    /// (HCL `block`) build their name from concatenated `identifier` /
    /// `string_lit` children. Everything else looks up the conventional
    /// `name` / `property` / `type` / `key` field.
    fn scope_name(&self, node: Node, is_call_promoted: bool, is_positional: bool) -> String {
        if is_call_promoted {
            return self.call_expression_name(node);
        }
        if is_positional {
            return self.positional_name(node);
        }
        // `property` covers JS `field_definition`'s name field.
        node.child_by_field_name("name")
            .or_else(|| node.child_by_field_name("property"))
            .or_else(|| node.child_by_field_name("type"))
            .or_else(|| node.child_by_field_name("key"))
            .and_then(|name_node| name_node.utf8_text(&self.source).ok())
            .unwrap_or("")
            .to_string()
    }

    /// Build a breadcrumb name from a node's leading positional children.
    /// Walks direct named children in source order, collects those whose
    /// kind is `identifier` or `string_lit`, and joins their source text
    /// with single spaces. Stops at the first child of any other kind so
    /// the body (`{ … }`) doesn't leak into the name. Used for HCL
    /// `block` nodes (`resource "aws_s3_bucket" "logs"`,
    /// `variable "region"`, `module "vpc"`).
    fn positional_name(&self, node: Node) -> String {
        let mut cursor = node.walk();
        let mut parts: Vec<&str> = Vec::new();
        for child in node.named_children(&mut cursor) {
            if !matches!(child.kind(), "identifier" | "string_lit") {
                break;
            }
            if let Ok(text) = child.utf8_text(&self.source) {
                parts.push(text);
            }
        }
        parts.join(" ")
    }

    /// True when the call node has a function-body kind in its arguments
    /// or in its `block` field (Ruby's `do_block` / brace `block` /
    /// `lambda` sit on the call directly, not inside `arguments`).
    /// Gates promotion of a call to a named structure: a labeled
    /// callback like `it("…", () => {})` or `it "…" do … end` is
    /// promoted; a plain `xs.length` / `1.to_s` call is not.
    fn has_function_argument(&self, node: Node) -> bool {
        self.field_contains_function_body(node, "arguments")
            || self.field_contains_function_body(node, "block")
    }

    /// True when `field` on `node` either *is* a function-body kind, or
    /// wraps one as a direct named child. Handles both shapes:
    /// JS/Go/Lua put the function inside an `argument_list`/`arguments`
    /// wrapper; Ruby's `block` field is the function body itself.
    fn field_contains_function_body(&self, node: Node, field: &str) -> bool {
        let Some(target) = node.child_by_field_name(field) else {
            return false;
        };
        if self.function_body_kinds.contains(&target.kind()) {
            return true;
        }
        let mut cursor = target.walk();
        target
            .named_children(&mut cursor)
            .any(|c| self.function_body_kinds.contains(&c.kind()))
    }

    /// Synthesise a breadcrumb name for a labeled call:
    /// `<callee>("<label>")` when the first positional argument is a
    /// string literal, just `<callee>` otherwise. Field conventions vary:
    /// JS/Go use `function`, Lua uses `name`, Ruby uses `method` (with an
    /// optional `receiver` to form `Receiver.method`). The callee text is
    /// taken verbatim from the source.
    fn call_expression_name(&self, node: Node) -> String {
        let callee = self.call_callee_text(node);
        let label = self.call_first_string_label(node);
        match label {
            Some(text) => format!("{callee}({text})"),
            None => callee,
        }
    }

    fn call_callee_text(&self, node: Node) -> String {
        // Ruby: optional `receiver` plus required `method`.
        if let Some(method) = node.child_by_field_name("method") {
            let method_text = method.utf8_text(&self.source).unwrap_or("");
            if let Some(receiver) = node.child_by_field_name("receiver") {
                let receiver_text = receiver.utf8_text(&self.source).unwrap_or("");
                return format!("{receiver_text}.{method_text}");
            }
            return method_text.to_string();
        }
        // JS / TS / Go.
        if let Some(func) = node.child_by_field_name("function") {
            return func.utf8_text(&self.source).unwrap_or("").to_string();
        }
        // Lua.
        if let Some(name) = node.child_by_field_name("name") {
            return name.utf8_text(&self.source).unwrap_or("").to_string();
        }
        String::new()
    }

    fn call_first_string_label(&self, node: Node) -> Option<String> {
        let args = node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let first = args.named_children(&mut cursor).next()?;
        if is_string_literal_kind(first.kind()) {
            first.utf8_text(&self.source).ok().map(String::from)
        } else {
            None
        }
    }

    /// True when any ancestor of `node` introduces a function body, per
    /// `function_body_kinds`. Demotes local helpers (`fn inner` inside
    /// `fn outer`, `const inner = () => {}` inside a method body) so they
    /// do not steal the hunk anchor from the enclosing named container.
    /// Class members like `method_definition` directly under `class_body`
    /// are not nested in a function body and remain anchors.
    fn is_nested_in_function(&self, node: Node) -> bool {
        // A labeled call-promoted scope (e.g. `t.Run("…", …)`,
        // `it("…", …)`, `app.get("/…", …)`) is itself a
        // unit-of-behaviour anchor, never a local helper of any
        // enclosing named function. Skip the walk for these.
        if self.is_labeled_call_promoted(node) {
            return false;
        }
        let mut cur = node.parent();
        while let Some(p) = cur {
            // Anchor-only function bodies (`arrow_function`, Lua
            // `function_definition`, Ruby `do_block`/`block`/`lambda`)
            // do not demote anything: nested labeled callbacks are
            // siblings, not local helpers.
            let pkind = p.kind();
            let parent_is_function_body = self.function_body_kinds.contains(&pkind);
            let parent_is_anchor_only = self.anchor_only_kinds.contains(&pkind);
            if parent_is_function_body && !parent_is_anchor_only {
                return true;
            }
            // A labeled call-promoted ancestor shields anything inside
            // it from being demoted by a *further-out* named function
            // (e.g. `t.Run(…, func() {…})` inside
            // `func TestX(t *testing.T) {…}` — the inner func_literal
            // and its descendants are not helpers of `TestX`).
            if self.is_labeled_call_promoted(p) {
                return false;
            }
            cur = p.parent();
        }
        false
    }

    /// True when `node` is a call kind that has been promoted to a
    /// structure (it has a function-body argument) *and* carries a
    /// string-literal label as its first positional argument. The label
    /// is what marks the call as a unit of behaviour rather than a
    /// transient computation (`xs.map(x => …)` is unlabeled and stays
    /// subject to demotion).
    fn is_labeled_call_promoted(&self, node: Node) -> bool {
        self.call_promoted_kinds.contains(&node.kind())
            && self.has_function_argument(node)
            && self.call_first_string_label(node).is_some()
    }

    fn point_at_first_non_whitespace(&self, line: usize) -> Point {
        let column = self
            .source_line_raw(line)
            .map(|text| {
                let trimmed = text.trim_start_matches(|c: char| c.is_whitespace());
                text.len().saturating_sub(trimmed.len())
            })
            .unwrap_or(0);
        Point::new(line, column)
    }

    /// Return the 0-indexed source line with original indentation preserved.
    fn source_line_raw(&self, line: usize) -> Option<String> {
        let text = std::str::from_utf8(&self.source).ok()?;
        text.lines().nth(line).map(|l| l.to_string())
    }
}

/// 1-indexed last source line covered by `node`.
///
/// tree-sitter ranges are end-exclusive: when a node ends at column 0 of a
/// row (e.g. a TOML `[bar]` table that ends at the `[` of the following
/// `[another]`), that row is not part of the node. Without this adjustment
/// `end_line` over-counts by one and hunks bleed into the next sibling.
fn node_end_line(node: Node<'_>) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > 0 {
        end.row
    } else {
        end.row + 1
    }
}

/// True for tree-sitter node kinds that represent a string literal
/// across the languages we support (JS/TS, Go, Lua, Ruby). Used to pick
/// the breadcrumb label for a labeled call: the first positional
/// argument is treated as a label only when its kind matches one here.
fn is_string_literal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "string" | "template_string" | "interpreted_string_literal" | "raw_string_literal"
    )
}

/// Walk backward through preceding `decorator` siblings of `node` and
/// return the earliest start row. When a class or method is decorated,
/// this extends the scope's start line to cover the decorators so hunks
/// for changes inside decorator arguments include the full decorator
/// context.
fn decorator_adjusted_start(node: Node<'_>) -> usize {
    let mut start = node.start_position().row;
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        if p.kind() == "decorator" {
            start = p.start_position().row;
            prev = p.prev_named_sibling();
        } else {
            break;
        }
    }
    start
}

/// If `node` (or any of its ancestors) is one of `kinds`, walk forward
/// through subsequent named siblings of the same kinds until landing on
/// a non-matching node. That node is the structure the leading
/// comments / attributes document; `enclosing_scopes` then walks *its*
/// ancestors for the breadcrumb.
///
/// Mirrors [`skip_decorators`] for `decorator` nodes. Returns `node`
/// unchanged when `kinds` is empty or `node` is not inside a leading
/// comment / attribute.
fn skip_leading_comments<'a>(node: Node<'a>, kinds: &[&str]) -> Node<'a> {
    if kinds.is_empty() {
        return node;
    }
    let mut leading: Option<Node<'a>> = None;
    let mut cur = Some(node);
    while let Some(c) = cur {
        if kinds.contains(&c.kind()) {
            leading = Some(c);
        }
        cur = c.parent();
    }
    let Some(mut l) = leading else {
        return node;
    };
    while kinds.contains(&l.kind()) {
        if let Some(sib) = l.next_named_sibling() {
            l = sib;
            continue;
        }
        // No more siblings inside this parent. If the parent itself has a
        // following sibling, attach to it: covers languages where a comment
        // between two structures is parsed as a trailing child of the
        // previous one (TOML tables, where `# leading for [bar]` lands
        // inside `[other]`).
        match l.parent().and_then(|p| p.next_named_sibling()) {
            Some(parent_next) => l = parent_next,
            None => return l,
        }
    }
    l
}

/// If `node` (or any of its ancestors) is a `decorator`, jump forward to
/// the structure that decorator decorates. For chained decorators we walk
/// through every named sibling that is itself a decorator, so we land on
/// the decorated structure regardless of how many decorators precede it.
///
/// Returns `node` unchanged if no enclosing decorator is found.
fn skip_decorators(node: Node) -> Node {
    let mut decorator: Option<Node> = None;
    let mut cur = Some(node);
    while let Some(c) = cur {
        if c.kind() == "decorator" {
            decorator = Some(c);
            break;
        }
        cur = c.parent();
    }
    let Some(mut d) = decorator else {
        return node;
    };
    while d.kind() == "decorator" {
        let Some(sib) = d.next_named_sibling() else {
            return d;
        };
        d = sib;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_file_parse_returns_some_for_known_extension() {
        assert!(ParsedFile::parse("src/main.rs", "fn main() {}\n").is_some());
    }

    #[test]
    fn parsed_file_parse_returns_none_for_unknown_extension() {
        assert!(ParsedFile::parse("data.xyz", "content").is_none());
    }

    #[test]
    fn parsed_file_parse_returns_none_for_no_extension() {
        assert!(ParsedFile::parse("Makefile", "all: build").is_none());
    }

    #[test]
    fn parsed_file_parse_uses_exact_filename() {
        assert!(ParsedFile::parse("Gemfile", "source 'https://rubygems.org'\n").is_some());
    }

    #[test]
    fn parsed_file_parse_uses_shebang_for_no_extension() {
        let source = "#!/usr/bin/env python3\n\ndef run():\n    return 1\n";
        let parsed = ParsedFile::parse("script", source).expect("parse from shebang");
        let scopes = parsed.enclosing_scopes(3);

        assert!(
            scopes
                .iter()
                .any(|scope| scope.kind == "function_definition")
        );
    }

    #[test]
    fn is_structure_true_for_function_item_in_rust() {
        let parsed = ParsedFile::parse("src/x.rs", "fn main() {}\n").expect("parse");
        let scopes = parsed.enclosing_scopes(0);
        let func = scopes
            .iter()
            .find(|s| s.kind == "function_item")
            .expect("function_item scope");
        assert!(parsed.is_structure(func));
        assert!(!parsed.is_data(func));
    }

    #[test]
    fn is_data_true_for_object_in_javascript() {
        let source = "\
const config = {
    name: \"test\",
    version: 1,
};
";
        let parsed = ParsedFile::parse("app.js", source).expect("parse");
        // line 1 is `    name: "test",` inside the object literal
        let scopes = parsed.enclosing_scopes(1);
        let object = scopes
            .iter()
            .find(|s| s.kind == "object")
            .expect("object scope");
        assert!(parsed.is_data(object));
        assert!(!parsed.is_structure(object));
    }

    #[test]
    fn enclosing_scopes_returns_outermost_first_chain() {
        let source = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        42
    }
}
";
        let parsed = ParsedFile::parse("src/lib.rs", source).expect("parse");
        // line 4 (0-indexed) is `        42` inside compute inside impl Foo
        let scopes = parsed.enclosing_scopes(4);
        let kinds: Vec<&str> = scopes.iter().map(|s| s.kind.as_str()).collect();
        let names: Vec<&str> = scopes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(kinds, vec!["impl_item", "function_item"]);
        assert_eq!(names, vec!["Foo", "compute"]);
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
            assert!(
                ParsedFile::parse(path, source).is_some(),
                "failed to parse {path}"
            );
        }
    }
}
