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
        let node = skip_decorators(node);

        let mut ancestors = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            let kind_is_structure = self.structure_kinds.contains(&n.kind());
            let kind_is_data = self.data_kinds.contains(&n.kind());
            let kind_is_promoted = self.promoted_kinds.contains(&n.kind());
            let include = kind_is_structure
                || kind_is_data
                || (kind_is_promoted && self.has_function_value(&n));
            // Demote structures and promoted scopes that live inside another
            // function body. They're local helpers, not anchors. Data scopes
            // (objects/arrays) are unaffected; their existing
            // outermost-fit logic already handles nesting sensibly.
            let include = include
                && !((kind_is_structure || kind_is_promoted) && self.is_nested_in_function(n));
            if include {
                let start_line = n.start_position().row + 1;
                let end_line = n.end_position().row + 1;
                // `property` covers JS `field_definition`'s name field name.
                let name = n
                    .child_by_field_name("name")
                    .or_else(|| n.child_by_field_name("property"))
                    .or_else(|| n.child_by_field_name("type"))
                    .or_else(|| n.child_by_field_name("key"))
                    .and_then(|name_node| name_node.utf8_text(&self.source).ok())
                    .unwrap_or("")
                    .to_string();
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
        self.structure_kinds.contains(&kind) || self.promoted_kinds.contains(&kind)
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

    /// True when any ancestor of `node` introduces a function body, per
    /// `function_body_kinds`. Demotes local helpers (`fn inner` inside
    /// `fn outer`, `const inner = () => {}` inside a method body) so they
    /// do not steal the hunk anchor from the enclosing named container.
    /// Class members like `method_definition` directly under `class_body`
    /// are not nested in a function body and remain anchors.
    fn is_nested_in_function(&self, node: Node) -> bool {
        let mut cur = node.parent();
        while let Some(p) = cur {
            if self.function_body_kinds.contains(&p.kind()) {
                return true;
            }
            cur = p.parent();
        }
        false
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
