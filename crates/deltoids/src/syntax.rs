//! Tree-sitter parsing and scope queries.
//!
//! Parses source code for a detected [`Language`]. All syntax-level
//! questions (enclosing scopes, breadcrumbs, expansion anchors) are answered
//! through methods on [`ParsedFile`].
//!
//! The ancestor chain (breadcrumb) is built from *boundary* node kinds:
//! named structures (functions, classes, methods, modules) plus the
//! promoted / labeled-callback / anchor-only refinements. Every other
//! ancestor is transparent. Growing a hunk's context outward through
//! transparent ancestors is [`ParsedFile::expansion_anchor`], which walks
//! the raw tree-sitter chain rather than this filtered breadcrumb chain.

use std::ops::Range;

use tree_sitter::{Node, Parser, Point, Tree};

use crate::Language;
use crate::language::{NameRule, TreeSitterConfig};

mod outline;

use crate::scope::ScopeNode;
pub(crate) use outline::{Definition, Outline, TopStream};

/// The grammar fields scope queries read.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Field {
    Name,
    Property,
    Type,
    Key,
    Body,
    Value,
    Right,
    Left,
    Declarator,
    Block,
    Arguments,
    Receiver,
    Trait,
    Method,
    Function,
    Object,
    HeadingContent,
}

const FIELD_NAMES: [&str; 17] = [
    "name",
    "property",
    "type",
    "key",
    "body",
    "value",
    "right",
    "left",
    "declarator",
    "block",
    "arguments",
    "receiver",
    "trait",
    "method",
    "function",
    "object",
    "heading_content",
];

/// Each [`Field`]'s id in one grammar, if the grammar has it.
struct FieldIds([Option<std::num::NonZeroU16>; 17]);

impl FieldIds {
    fn of(language: &tree_sitter::Language) -> Self {
        FieldIds(FIELD_NAMES.map(|name| language.field_id_for_name(name)))
    }
}

/// A parsed source file: its syntax tree, the source it came from, and the
/// language taxonomy needed to interpret nodes.
pub(crate) struct ParsedFile {
    tree: Tree,
    /// Field ids of the grammar, looked up once: tree-sitter finds a field
    /// by name with a scan.
    fields: FieldIds,
    source: String,
    /// Byte span of each source line, as split by `str::lines`.
    lines: Vec<Range<usize>>,
    config: TreeSitterConfig,
}

impl ParsedFile {
    /// Parse `source` for the language detected from `path`.
    #[cfg(test)]
    pub(crate) fn parse(path: &str, source: &str) -> Option<Self> {
        Self::parse_as(Language::detect(path, source)?, source)
    }

    pub(crate) fn parse_as(language: Language, source: &str) -> Option<Self> {
        let config = language.tree_sitter_config();
        let mut parser = Parser::new();
        parser.set_language(&config.language.into()).ok()?;
        let tree = parser.parse(source, None)?;
        let lines = source
            .lines()
            .map(|line| {
                let start = line.as_ptr() as usize - source.as_ptr() as usize;
                start..start + line.len()
            })
            .collect();
        Some(ParsedFile {
            fields: FieldIds::of(&tree.language()),
            tree,
            source: source.to_string(),
            lines,
            config,
        })
    }

    /// `node`'s child in `field`.
    pub(crate) fn child<'a>(&self, node: Node<'a>, field: Field) -> Option<Node<'a>> {
        let id = self.fields.0[field as usize]?;
        node.child_by_field_id(id.get())
    }

    /// Return all enclosing *boundary* scope nodes at the given 0-indexed
    /// `line`, outermost first — the breadcrumb chain. A scope is included
    /// when its tree-sitter kind is a named structure, an anchor-only
    /// callback, or a wrapper kind promoted because its value/argument is
    /// a function body. Transparent ancestors (assignments, calls,
    /// argument lists, anonymous data containers) are not included; growing
    /// context through them is [`Self::expansion_anchor`]. Structures and
    /// promoted scopes nested inside another function body (local helpers)
    /// are excluded.
    pub fn enclosing_scopes(&self, line: usize) -> Vec<ScopeNode> {
        self.enclosing_scope_nodes(line)
            .into_iter()
            .map(|n| self.scope_node_from(n))
            .collect()
    }

    /// Innermost enclosing scope at `line` (the last entry of
    /// [`Self::enclosing_scopes`]), regardless of size.
    pub(crate) fn innermost_structure(&self, line: usize) -> Option<ScopeNode> {
        let node = *self.enclosing_scope_nodes(line).last()?;
        Some(self.scope_node_from(node))
    }

    /// Line spans enclosing `line`, in the order a manual expansion tries
    /// them: boundary scopes innermost first, then every raw tree-sitter
    /// ancestor below the root, innermost first. Spans are 0-based,
    /// end-exclusive, and start at leading comments and decorators.
    pub(crate) fn enclosing_spans(&self, line: usize) -> Vec<Range<usize>> {
        let mut spans: Vec<Range<usize>> = self
            .enclosing_scopes(line)
            .iter()
            .rev()
            .map(ScopeNode::lines)
            .collect();
        let mut current = self.resolve_start_node(line);
        while let Some(node) = current.filter(|node| node.parent().is_some()) {
            spans.push(
                leading_adjusted_start(node, self.config.leading_comment_kinds)
                    ..node_end_line(node),
            );
            current = node.parent();
        }
        spans
    }

    /// Number of source lines.
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Byte ranges, relative to the line's start, of the identifier
    /// leaves on 0-based `line`, in source order. Identifiers are leaves
    /// whose kind ends in `identifier` (`field_identifier`,
    /// `type_identifier`, …) plus Ruby's `constant`.
    pub(crate) fn identifiers_on_line(&self, line: usize) -> Vec<Range<usize>> {
        let Some(span) = self.lines.get(line) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut stack = vec![self.tree.root_node()];
        while let Some(node) = stack.pop() {
            let (start, end) = (node.start_position().row, node.end_position().row);
            if start > line || end < line {
                continue;
            }
            if node.child_count() > 0 {
                let mut cursor = node.walk();
                let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev());
                continue;
            }
            let kind = node.kind();
            if start == end && (kind.ends_with("identifier") || kind == "constant") {
                found.push(node.start_byte() - span.start..node.end_byte() - span.start);
            }
        }
        found
    }

    /// Collect the enclosing *boundary* tree-sitter nodes at `line`,
    /// outermost first. This is the shared walk behind
    /// [`Self::enclosing_scopes`] (which maps each node to a [`ScopeNode`])
    /// and [`Self::breadcrumb_scopes`] (which additionally drops nodes that
    /// carry no breadcrumb-worthy name). Keeping both callers on one walk
    /// keeps the enclosing and breadcrumb chains in lockstep.
    fn enclosing_scope_nodes(&self, line: usize) -> Vec<Node<'_>> {
        let Some(node) = self.resolve_start_node(line) else {
            return Vec::new();
        };

        let mut ancestors = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            // Scopes inside another function body are local helpers, not
            // scopes of their own (see [`Self::is_nested_in_function`]).
            if self.is_scope_kind(n) && !self.is_nested_in_function(n) {
                ancestors.push(n);
            }
            current = n.parent();
        }

        ancestors.reverse();
        ancestors
    }

    /// Whether `node` is a scope by its kind: a structure, an anonymous
    /// callback, a wrapper whose value is a function, or a call with a
    /// function argument. [`Self::is_nested_in_function`] then demotes the
    /// ones inside another function body.
    fn is_scope_kind(&self, n: Node) -> bool {
        let kind = n.kind();
        self.config.structure_kinds.contains(&kind)
            || self.config.anchor_only_kinds.contains(&kind)
            || (self.config.promoted_kinds.contains(&kind) && self.has_function_value(&n))
            || (self.config.call_promoted_kinds.contains(&kind) && self.has_function_argument(n))
    }

    /// Whether a scope shows in breadcrumbs: anonymous callbacks and calls
    /// whose only argument is their callback do not (see
    /// [`Self::breadcrumb_scopes`]).
    fn shows_in_breadcrumb(&self, n: Node) -> bool {
        !self.config.anchor_only_kinds.contains(&n.kind()) && !self.is_block_only_call(n)
    }

    /// Enclosing scopes filtered down to breadcrumb boundaries at `line`,
    /// outermost first. Starts from the same boundary chain as
    /// [`Self::enclosing_scopes`], then drops nodes that carry no name to
    /// display:
    ///
    /// - anchor-only callbacks (anonymous `do_block` / `arrow_function`):
    ///   their identity is the call signature already visible as the hunk's
    ///   first context line.
    /// - block-only call-promoted wrappers (`expect { … }`, `subject { … }`,
    ///   `beforeEach(() => {})`, `withDORetry(() => {})`, `xs.map(x => …)`):
    ///   the block/callback is their only argument, so they are restructured
    ///   expressions, not named units. See [`Self::is_block_only_call`].
    ///
    /// Call-promoted wrappers that carry an identifying argument
    /// (`it "…" do`, `namespace :github do`, `task sync: :environment do`,
    /// `RSpec.describe UserService do`) are kept: breadcrumbs accept any
    /// non-block argument as a label, not just a string.
    pub fn breadcrumb_scopes(&self, line: usize) -> Vec<ScopeNode> {
        self.enclosing_scope_nodes(line)
            .into_iter()
            .filter(|n| self.shows_in_breadcrumb(*n))
            .map(|n| self.scope_node_from(n))
            .collect()
    }

    /// Build a [`ScopeNode`] from a tree-sitter node, resolving its
    /// breadcrumb name, leading-comment-adjusted start, and end line the
    /// same way [`Self::enclosing_scopes`] does.
    fn scope_node_from(&self, n: Node) -> ScopeNode {
        let start_line = leading_adjusted_start(n, self.config.leading_comment_kinds) + 1;
        let end_line = node_end_line(n);
        let name = self.scope_name(n);
        let text = self.source_line(n.start_position().row).unwrap_or_default();
        ScopeNode {
            kind: n.kind().to_string(),
            name,
            start_line,
            end_line,
            text: text.to_string(),
        }
    }

    /// Resolve the tree-sitter node a scope query starts from for `line`.
    ///
    /// Descends to the node at the first non-whitespace column, then skips
    /// leading comments / attributes and decorators so doc comments,
    /// attributes, and decorators attach to the structure they document
    /// rather than its parent. Shared by [`Self::enclosing_scopes`] and
    /// [`Self::expansion_anchor`].
    ///
    // Method- and class-level decorators (e.g. `@Cron(...)` above a class
    // method or `@Injectable()` above a class) appear in the tree as
    // siblings of the decorated node, not as children. A query at a
    // decorator line would otherwise walk up through `class_body` straight
    // to the class, skipping the method the decorator belongs to.
    //
    // Doc comments (`///`, JSDoc, `# …` above a `def`, …) and Rust
    // attributes (`#[derive(…)]`) sit in the same shape: a sibling above
    // the structure they document, not a child. `skip_leading_comments`
    // walks past such siblings to the structure they precede.
    fn resolve_start_node(&self, line: usize) -> Option<Node<'_>> {
        let point = self.point_at_first_non_whitespace(line);
        let node = self
            .tree
            .root_node()
            .descendant_for_point_range(point, point)?;
        let skipped = skip_leading_comments(node, self.config.leading_comment_kinds);
        // Landing on a wrapper (`export const f = () => {}`) hides the
        // scope: it is a *descendant* of the wrapper, and the caller only
        // walks upward from here. Descend back into it.
        let skipped = if skipped.id() == node.id() {
            skipped
        } else {
            self.wrapped_scope(skipped)
        };
        Some(skip_decorators(skipped))
    }

    /// Innermost scope nested inside `node` that starts on the same source
    /// row, or `node` when there is none.
    ///
    /// Inverse of [`same_row_wrapper`]: `export const f = () => {}` parses
    /// as `export_statement` > `lexical_declaration` >
    /// `variable_declarator`, all starting on the same row, and only the
    /// declarator is the function scope. Only structures and promoted
    /// kinds with a function value qualify, so a plain `const a = 1`
    /// wrapper is left alone.
    fn wrapped_scope<'a>(&self, node: Node<'a>) -> Node<'a> {
        let row = node.start_position().row;
        let mut best = node;
        let mut current = node;
        while let Some(child) = current
            .named_child(0)
            .filter(|c| c.start_position().row == row)
        {
            let kind_is_promoted = self.config.promoted_kinds.contains(&child.kind());
            if self.config.structure_kinds.contains(&child.kind())
                || (kind_is_promoted && self.has_function_value(&child))
            {
                best = child;
            }
            current = child;
        }
        best
    }

    /// Pick an expansion anchor for a *contained* change when no enclosing
    /// structure exists. Grows the hunk outward through transparent
    /// ancestors (assignments, calls, argument lists, literals —
    /// kind-agnostic) and stops just below the file root.
    ///
    /// Walks the raw tree-sitter ancestor chain from the change start
    /// (resolved exactly as [`Self::enclosing_scopes`]), excludes the file
    /// root, and returns the **outermost** ancestor that contains the
    /// change range `[range_start, range_end)` (0-indexed, end-exclusive)
    /// and whose span fits within `budget` lines. The anchor must span
    /// more lines than the change itself; otherwise returns `None` so
    /// trivial single-line entry nodes (a one-line `array`, a JSON `pair`)
    /// fall through to the default 3-line context.
    ///
    /// The returned [`ScopeNode`] carries the raw node kind and an empty
    /// name. It is never a structure, so the breadcrumb filter drops it.
    ///
    /// No ceiling logic is needed: callers only invoke this when there is
    /// no enclosing structure, so the only thing above the chain is the
    /// root, which is already excluded.
    ///
    /// Returns `None` for prose languages (Markdown) where transparent
    /// expansion is disabled: their outermost fitting ancestor is a whole
    /// heading-delimited section that would swallow unrelated sibling
    /// blocks, so such changes use 3-line default context instead.
    pub fn expansion_anchor(
        &self,
        range_start: usize,
        range_end: usize,
        budget: usize,
    ) -> Option<ScopeNode> {
        if !self.config.transparent_expansion {
            return None;
        }
        let node = self.resolve_start_node(range_start)?;
        let change_lines = range_end.saturating_sub(range_start).max(1);

        // Raw ancestor chain, innermost first, excluding the file root.
        // An ancestor is kept only when its own parent exists; the
        // top-level statement's parent is the root, so it is included,
        // but the root itself is not.
        let mut chain: Vec<Node<'_>> = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            if n.parent().is_some() {
                chain.push(n);
            }
            current = n.parent();
        }

        // Outermost first: the largest spans come first, so the first
        // ancestor that both contains the change and fits the budget is
        // the outermost fitting ancestor.
        let anchor = chain.into_iter().rev().find(|n| {
            let start = leading_adjusted_start(*n, self.config.leading_comment_kinds);
            let end = node_end_line(*n).saturating_sub(1);
            let lines = end - start + 1;
            let contains = start <= range_start && end >= range_end.saturating_sub(1);
            contains && lines <= budget
        })?;

        let start_line = leading_adjusted_start(anchor, self.config.leading_comment_kinds) + 1;
        let end_line = node_end_line(anchor);
        // Floor: the anchor must add context beyond the change itself.
        if end_line.saturating_sub(start_line) < change_lines {
            return None;
        }
        let text = self
            .source_line(anchor.start_position().row)
            .unwrap_or_default();
        Some(ScopeNode {
            kind: anchor.kind().to_string(),
            name: String::new(),
            start_line,
            end_line,
            text: text.to_string(),
        })
    }

    /// True when `node`'s `value` field holds a function body (arrow
    /// function, function expression, named function, or any other kind
    /// listed in `function_body_kinds`). Gates promotion of wrapper kinds
    /// (class fields, variable declarators) to structures.
    fn has_function_value(&self, node: &Node) -> bool {
        let Some(value) = self.child(*node, Field::Value) else {
            return false;
        };
        self.config.function_body_kinds.contains(&value.kind())
    }

    /// Resolve the breadcrumb name for a scope node. Call-promoted nodes
    /// (e.g. `call_expression` in JS/TS) build their name from the callee
    /// plus the first string-literal argument. Kinds with a [`NameRule`]
    /// follow it. Everything else looks up the conventional `name` /
    /// `property` / `type` / `key` field.
    fn scope_name(&self, node: Node) -> String {
        if self.config.call_promoted_kinds.contains(&node.kind()) {
            return self.call_expression_name(node);
        }
        match self.config.name_rule(node.kind()) {
            Some(NameRule::Positional) => self.positional_name(node),
            Some(NameRule::ObjectReference) => self.object_reference_name(node),
            Some(NameRule::Declarator) => self.declarator_name(node),
            Some(NameRule::ImplHeader) => self.impl_name(node),
            Some(NameRule::Receiver) => self.receiver_name(node),
            Some(NameRule::SingletonMethod) => self.singleton_method_name(node),
            Some(NameRule::Specs) => self.specs_name(node),
            Some(NameRule::Header) => self.header_name(node),
            Some(NameRule::TableKey) => self.table_key_name(node),
            Some(NameRule::HeadingText) => self.field_text(node, Field::HeadingContent),
            Some(NameRule::SectionHeading) => self.section_name(node),
            Some(NameRule::Property) => self.property_name(node),
            Some(NameRule::ChildKind(kind)) => self
                .child_of_kind(node, kind)
                .map(|child| self.collapsed_text(child))
                .unwrap_or_default(),
            Some(NameRule::Field(field)) => node
                .child_by_field_name(field)
                .map(|child| self.collapsed_text(child))
                .unwrap_or_default(),
            Some(NameRule::QuotedKey) => {
                let key = self.field_text(node, Field::Key);
                let unquoted = key
                    .strip_prefix('"')
                    .and_then(|k| k.strip_suffix('"'))
                    .or_else(|| key.strip_prefix('\'').and_then(|k| k.strip_suffix('\'')));
                unquoted.map_or(key.clone(), str::to_string)
            }
            // `property` covers JS `field_definition`'s name field.
            None => [Field::Name, Field::Property, Field::Type, Field::Key]
                .into_iter()
                .find_map(|field| self.child(node, field))
                .map(|name| self.collapsed_text(name))
                .unwrap_or_default(),
        }
    }

    /// `node`'s source with every run of whitespace collapsed to one
    /// space, so names written across lines read as one line.
    fn collapsed_text(&self, node: Node) -> String {
        collapse_whitespace(node.utf8_text(self.source.as_bytes()).unwrap_or(""))
    }

    /// The collapsed text of `node`'s `field` child; empty without one.
    fn field_text(&self, node: Node, field: Field) -> String {
        self.child(node, field)
            .map(|child| self.collapsed_text(child))
            .unwrap_or_default()
    }

    /// The name a C or C++ function declares: down the `declarator` chain
    /// (function, pointer, reference, and parenthesized declarators) to
    /// the first node that is not itself a declarator, read as written.
    /// `char *name(void)` is `name`; `int& Foo::ref()` is `Foo::ref`;
    /// `void (*handler(int))(int)` is `handler`.
    fn declarator_name(&self, node: Node) -> String {
        let mut current = self.child(node, Field::Declarator);
        while let Some(declarator) = current {
            if !declarator.kind().ends_with("declarator") {
                return self.collapsed_text(declarator);
            }
            // Reference and parenthesized declarators hold the inner
            // declarator as a plain named child, not a field.
            current = self
                .child(declarator, Field::Declarator)
                .or_else(|| declarator.named_child(0));
        }
        String::new()
    }

    /// A Rust impl block: `Trait for Type`, or `Type` for an inherent
    /// impl. Type arguments stay: `From<T> for Wrapper<T>`.
    fn impl_name(&self, node: Node) -> String {
        let ty = self.field_text(node, Field::Type);
        match self.child(node, Field::Trait) {
            Some(tr) => format!("{} for {ty}", self.collapsed_text(tr)),
            None => ty,
        }
    }

    /// A Go method: `Type.Method`, the receiver type without its pointer
    /// or type arguments, so `func (l *List[T]) Push` is `List.Push`.
    fn receiver_name(&self, node: Node) -> String {
        let name = self.field_text(node, Field::Name);
        let receiver = self
            .child(node, Field::Receiver)
            .and_then(|list| list.named_child(0))
            .and_then(|parameter| self.child(parameter, Field::Type))
            .map(|ty| self.collapsed_text(ty));
        let Some(receiver) = receiver else {
            return name;
        };
        let ty = receiver.trim_start_matches(['*', ' ']);
        let ty = ty.split('[').next().unwrap_or(ty).trim();
        format!("{ty}.{name}")
    }

    /// A Ruby singleton method: `receiver.name`, so `def self.build` is
    /// `self.build`.
    fn singleton_method_name(&self, node: Node) -> String {
        format!(
            "{}.{}",
            self.field_text(node, Field::Object),
            self.field_text(node, Field::Name)
        )
    }

    /// A Go `type` declaration: the names of the types it declares, in
    /// order, joined with `, `.
    fn specs_name(&self, node: Node) -> String {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter(|child| matches!(child.kind(), "type_spec" | "type_alias"))
            .map(|spec| self.field_text(spec, Field::Name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The text before the node's body: a CSS rule's selectors or an
    /// at-rule's header. The whole node when it has no body.
    fn header_name(&self, node: Node) -> String {
        let mut cursor = node.walk();
        let end = node
            .children(&mut cursor)
            .find(|child| self.config.body_kinds.contains(&child.kind()))
            .map_or(node.end_byte(), |body| body.start_byte());
        collapse_whitespace(&self.source[node.start_byte()..end])
    }

    /// A Markdown section: the text of the heading that opens it. Empty
    /// for the text before the first heading.
    fn section_name(&self, node: Node) -> String {
        node.named_child(0)
            .filter(|heading| heading.kind().ends_with("_heading"))
            .map(|heading| self.field_text(heading, Field::HeadingContent))
            .unwrap_or_default()
    }

    /// A Kotlin property: the name of the variable it declares, without
    /// its type.
    fn property_name(&self, node: Node) -> String {
        self.child_of_kind(node, "variable_declaration")
            .and_then(|declaration| declaration.named_child(0))
            .map(|name| self.collapsed_text(name))
            .unwrap_or_default()
    }

    /// The first named child of `kind`.
    fn child_of_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| child.kind() == kind)
    }

    /// A TOML table's key as written: bare, dotted, or quoted.
    fn table_key_name(&self, node: Node) -> String {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| child.kind().ends_with("key"))
            .map(|key| self.collapsed_text(key))
            .unwrap_or_default()
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
            if let Ok(text) = child.utf8_text(self.source.as_bytes()) {
                parts.push(text);
            }
        }
        parts.join(" ")
    }

    /// Breadcrumb name for a SQL DDL node: the text of its first
    /// descendant `object_reference` child. `create_table users` yields
    /// `users`, `create_table public.users` yields `public.users`. The
    /// statement node itself carries no `name` field, so the name is read
    /// from the nested reference. Returns an empty string when no
    /// `object_reference` is present.
    fn object_reference_name(&self, node: Node) -> String {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|c| c.kind() == "object_reference")
            .and_then(|reference| reference.utf8_text(self.source.as_bytes()).ok())
            .unwrap_or("")
            .to_string()
    }

    /// True when the call node has a function-body kind in its arguments
    /// or in its `block` field (Ruby's `do_block` / brace `block` /
    /// `lambda` sit on the call directly, not inside `arguments`).
    /// Gates promotion of a call to a named structure: a labeled
    /// callback like `it("…", () => {})` or `it "…" do … end` is
    /// promoted; a plain `xs.length` / `1.to_s` call is not.
    fn has_function_argument(&self, node: Node) -> bool {
        self.field_contains_function_body(node, Field::Arguments)
            || self.field_contains_function_body(node, Field::Block)
    }

    /// True when `field` on `node` either *is* a function-body kind, or
    /// wraps one as a direct named child. Handles both shapes:
    /// JS/Go/Lua put the function inside an `argument_list`/`arguments`
    /// wrapper; Ruby's `block` field is the function body itself.
    fn field_contains_function_body(&self, node: Node, field: Field) -> bool {
        let Some(target) = self.child(node, field) else {
            return false;
        };
        if self.config.function_body_kinds.contains(&target.kind()) {
            return true;
        }
        let mut cursor = target.walk();
        target
            .named_children(&mut cursor)
            .any(|c| self.config.function_body_kinds.contains(&c.kind()))
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
        if let Some(method) = self.child(node, Field::Method) {
            let method_text = method.utf8_text(self.source.as_bytes()).unwrap_or("");
            if let Some(receiver) = self.child(node, Field::Receiver) {
                let receiver_text = receiver.utf8_text(self.source.as_bytes()).unwrap_or("");
                return format!("{receiver_text}.{method_text}");
            }
            return method_text.to_string();
        }
        // JS / TS / Go.
        if let Some(func) = self.child(node, Field::Function) {
            return func
                .utf8_text(self.source.as_bytes())
                .unwrap_or("")
                .to_string();
        }
        // Lua.
        if let Some(name) = self.child(node, Field::Name) {
            return name
                .utf8_text(self.source.as_bytes())
                .unwrap_or("")
                .to_string();
        }
        String::new()
    }

    fn call_first_string_label(&self, node: Node) -> Option<String> {
        let args = self.child(node, Field::Arguments)?;
        let mut cursor = args.walk();
        let first = args.named_children(&mut cursor).next()?;
        if is_string_literal_kind(first.kind()) {
            first
                .utf8_text(self.source.as_bytes())
                .ok()
                .map(String::from)
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
            if let Some(nested) = self.function_scope(p) {
                return nested;
            }
            cur = p.parent();
        }
        false
    }

    /// What `node` says about whether what it holds sits in a function
    /// body: `Some(true)` for a named function body, `Some(false)` for a
    /// labeled call, `None` when it says nothing and its own ancestors
    /// decide.
    ///
    /// Anonymous function bodies (`arrow_function`, Lua
    /// `function_definition`, Ruby `do_block`/`block`/`lambda`) say
    /// nothing: nested labeled callbacks are siblings, not local helpers.
    /// A labeled call shields what it holds from a *further-out* named
    /// function (e.g. `t.Run(…, func() {…})` inside
    /// `func TestX(t *testing.T) {…}` — the inner func_literal and its
    /// descendants are not helpers of `TestX`).
    fn function_scope(&self, node: Node) -> Option<bool> {
        let kind = node.kind();
        if self.config.function_body_kinds.contains(&kind)
            && !self.config.anchor_only_kinds.contains(&kind)
        {
            return Some(true);
        }
        if self.is_labeled_call_promoted(node) {
            return Some(false);
        }
        None
    }

    /// True when `node` is a call kind that has been promoted to a
    /// structure (it has a function-body argument) *and* carries a
    /// string-literal label as its first positional argument. The label
    /// is what marks the call as a unit of behaviour rather than a
    /// transient computation (`xs.map(x => …)` is unlabeled and stays
    /// subject to demotion).
    fn is_labeled_call_promoted(&self, node: Node) -> bool {
        self.config.call_promoted_kinds.contains(&node.kind())
            && self.has_function_argument(node)
            && self.call_first_string_label(node).is_some()
    }

    /// True when `node`'s `arguments` field carries at least one named
    /// child that is not itself a function body. This is the looser
    /// "breadcrumb-worthy" test for a call-promoted node: any identifying
    /// positional/keyword argument (string, symbol, keyword pair,
    /// constant) marks the call as a named unit.
    ///
    /// `expect { … }` has no `arguments` field (its block sits on the
    /// `block` field), so this is false. `beforeEach(() => {})` has an
    /// `arguments` list whose only named child is the arrow — a function
    /// body — so this is also false.
    fn call_has_identifying_argument(&self, node: Node) -> bool {
        let Some(args) = self.child(node, Field::Arguments) else {
            return false;
        };
        let mut cursor = args.walk();
        args.named_children(&mut cursor)
            .any(|c| !self.config.function_body_kinds.contains(&c.kind()))
    }

    /// True when `node` is a call-promoted wrapper whose only argument is
    /// a block/callback (`expect { … }`, `subject { … }`,
    /// `beforeEach(() => {})`, `withDORetry(() => {})`, `xs.map(x => …)`).
    /// Such wrappers are restructured expressions, not named units, and
    /// must not appear as breadcrumb boundaries. See
    /// [`Self::breadcrumb_scopes`].
    fn is_block_only_call(&self, node: Node) -> bool {
        self.config.call_promoted_kinds.contains(&node.kind())
            && !self.call_has_identifying_argument(node)
    }

    fn point_at_first_non_whitespace(&self, line: usize) -> Point {
        let column = self
            .source_line(line)
            .map(|text| {
                let trimmed = text.trim_start_matches(|c: char| c.is_whitespace());
                text.len().saturating_sub(trimmed.len())
            })
            .unwrap_or(0);
        Point::new(line, column)
    }

    /// The 0-indexed source line with original indentation preserved.
    fn source_line(&self, line: usize) -> Option<&str> {
        self.lines.get(line).map(|span| &self.source[span.clone()])
    }
}

/// `text` with every run of whitespace collapsed to one space and the
/// ends trimmed.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// Walk backward through preceding named siblings that are decorators or
/// leading-comment kinds (doc comments, attributes) and return the earliest
/// start row. Extends the scope's start line so hunks for changes inside
/// the function body include the full leading context.
///
/// The walk starts from the outermost ancestor that begins on the same
/// row as `node` (see [`same_row_wrapper`]), not from `node` itself:
/// `export const f = () => {}` picks the promoted `variable_declarator`
/// as its scope, but the doc comment above it is a sibling of the
/// enclosing `export_statement`, two levels up.
fn leading_adjusted_start(node: Node<'_>, leading_comment_kinds: &[&str]) -> usize {
    let anchor = same_row_wrapper(node);
    let mut start = node.start_position().row;
    let mut prev = anchor.prev_named_sibling();
    while let Some(p) = prev {
        if p.kind() == "decorator" || is_leading_comment(p, leading_comment_kinds) {
            start = p.start_position().row;
            prev = p.prev_named_sibling();
        } else {
            break;
        }
    }
    start
}

/// Whether `node` is one of the leading-comment `kinds` and documents what
/// follows it. Rust inner doc comments (`//!`, `/*!`) share the comment
/// kinds but document the enclosing file or module.
fn is_leading_comment(node: Node<'_>, kinds: &[&str]) -> bool {
    kinds.contains(&node.kind()) && !is_inner_doc_comment(node)
}

/// Whether `node` is a Rust inner doc comment (`//!`, `/*!`): the grammar
/// marks it with an `inner` field. No other grammar has that field.
pub(crate) fn is_inner_doc_comment(node: Node<'_>) -> bool {
    node.child_by_field_name("inner").is_some()
}

/// Climb to the outermost ancestor that starts on the same source row as
/// `node`, stopping below the file root.
///
/// Such an ancestor is a wrapper sharing the scope's first line — the
/// `lexical_declaration` and `export_statement` around
/// `export const f = () => {}`, or the `export_statement` around
/// `export function f() {}`. Anything written above that wrapper belongs
/// to the scope, so leading-comment lookups anchor here. Wrappers on
/// their own rows (Python `decorated_definition`, Rust attributes) start
/// on a different row and are left to the sibling walk.
fn same_row_wrapper(node: Node<'_>) -> Node<'_> {
    let row = node.start_position().row;
    let mut anchor = node;
    while let Some(parent) = anchor.parent() {
        // The root spans the whole file; it is never a wrapper.
        if parent.parent().is_none() || parent.start_position().row != row {
            break;
        }
        anchor = parent;
    }
    anchor
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
        if is_leading_comment(c, kinds) {
            leading = Some(c);
        }
        cur = c.parent();
    }
    let Some(mut l) = leading else {
        return node;
    };
    while is_leading_comment(l, kinds) {
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
    fn breadcrumb_scopes_drops_block_only_expect() {
        let source = "\
RSpec.describe Client do
  it \"raises on repeated failure\" do
    expect {
      collect_pages(max)
    }.to raise_error(RetryError)
  end
end
";
        let parsed = ParsedFile::parse("spec.rb", source).expect("parse");
        // Line 3 (0-indexed) is `collect_pages(max)` inside the block-only
        // `expect { … }`. The chain must end at the labeled `it("…")`, with
        // no `[call expect]`.
        let scopes = parsed.breadcrumb_scopes(3);
        let names: Vec<&str> = scopes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["RSpec.describe", "it(\"raises on repeated failure\")"]
        );
    }

    #[test]
    fn breadcrumb_scopes_keeps_symbol_and_keyword_labeled_calls() {
        let source = "\
namespace :github do
  task sync: :environment do
    run_sync
  end
end
";
        let parsed = ParsedFile::parse("tasks.rake", source).expect("parse");
        // Line 2 (0-indexed) is `run_sync` inside `task sync: :environment`
        // inside `namespace :github`. Both carry an identifying argument
        // (a symbol / keyword pair), so both stay in the breadcrumb.
        let scopes = parsed.breadcrumb_scopes(2);
        let kinds: Vec<&str> = scopes.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, vec!["call", "call"]);
        assert_eq!(scopes[0].name, "namespace");
        assert_eq!(scopes[1].name, "task");
    }

    #[test]
    fn breadcrumb_scopes_keeps_string_and_constant_labeled_calls() {
        let source = "\
RSpec.describe UserService do
  it \"does a thing\" do
    perform
  end
end
";
        let parsed = ParsedFile::parse("spec.rb", source).expect("parse");
        // Line 2 (0-indexed) is `perform` inside `it "…"` inside
        // `RSpec.describe UserService`. A string label and a constant
        // argument both keep their call in the breadcrumb.
        let scopes = parsed.breadcrumb_scopes(2);
        let names: Vec<&str> = scopes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["RSpec.describe", "it(\"does a thing\")"]);
    }

    #[test]
    fn breadcrumb_scopes_drops_js_block_only_callback_keeps_labeled() {
        let source = "\
describe(\"suite\", () => {
  beforeEach(() => {
    setup();
  });
});
";
        let parsed = ParsedFile::parse("suite.test.js", source).expect("parse");
        // Line 2 (0-indexed) is `setup()` inside `beforeEach(() => {})`
        // (block-only callback) inside `describe("suite", …)` (labeled).
        // The block-only `beforeEach` drops; the labeled `describe` stays.
        let scopes = parsed.breadcrumb_scopes(2);
        let names: Vec<&str> = scopes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["describe(\"suite\")"]);
    }

    #[test]
    fn enclosing_scopes_omits_anonymous_data_containers() {
        let source = "\
const config = {
    name: \"test\",
    version: 1,
};
";
        let parsed = ParsedFile::parse("app.js", source).expect("parse");
        // line 1 is `    name: "test",` inside the object literal. The
        // object is a transparent container, not a breadcrumb boundary,
        // so it never appears in the enclosing-scope chain.
        let scopes = parsed.enclosing_scopes(1);
        assert!(scopes.iter().all(|s| s.kind != "object"));
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
    fn enclosing_scopes_start_at_leading_comment_of_exported_arrow_fn() {
        let source = "\
const prefix = 1;

// Doc one.
// Doc two.
export const compute = (a: number) => {
  return a;
};
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // line 5 (0-indexed) is `  return a;` inside the arrow body.
        let scope = parsed
            .enclosing_scopes(5)
            .into_iter()
            .find(|s| s.name == "compute")
            .expect("declarator scope");
        // 1-indexed line 3 is `// Doc one.`, the first leading comment.
        assert_eq!(scope.start_line, 3);
    }

    #[test]
    fn innermost_structure_at_comment_above_exported_arrow_fn_is_the_function() {
        let source = "\
const prefix = 1;

// Doc one.
export const compute = (a: number) => {
  return a;
};
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // line 2 (0-indexed) is the comment; it documents `compute`.
        let scope = parsed
            .innermost_structure(2)
            .expect("scope at comment line");
        assert_eq!(scope.kind, "variable_declarator");
        assert_eq!(scope.name, "compute");
    }

    #[test]
    fn innermost_structure_at_comment_above_exported_function_is_the_function() {
        let source = "\
const prefix = 1;

// Doc one.
export function compute(a: number) {
  return a;
}
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        let scope = parsed
            .innermost_structure(2)
            .expect("scope at comment line");
        assert_eq!(scope.kind, "function_declaration");
        assert_eq!(scope.name, "compute");
    }

    #[test]
    fn enclosing_scopes_start_at_leading_comment_of_local_arrow_fn() {
        let source = "\
const prefix = 1;

// Doc one.
const compute = (a: number) => {
  return a;
};
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // line 4 (0-indexed) is `  return a;` inside the arrow body.
        let scope = parsed
            .enclosing_scopes(4)
            .into_iter()
            .find(|s| s.name == "compute")
            .expect("declarator scope");
        assert_eq!(scope.start_line, 3);
    }

    fn scopes_at(source: &str, line: usize) -> Vec<ScopeNode> {
        ParsedFile::parse("test.rs", source)
            .expect("parse")
            .enclosing_scopes(line)
    }

    #[test]
    fn enclosing_scopes_extracts_name_for_various_kinds() {
        // struct
        let source = "struct MyStruct {\n    field: i32,\n}\n";
        let scopes = scopes_at(source, 1);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].kind, "struct_item");
        assert_eq!(scopes[0].name, "MyStruct");

        // enum
        let source = "enum Color {\n    Red,\n}\n";
        let scopes = scopes_at(source, 1);
        assert_eq!(scopes[0].kind, "enum_item");
        assert_eq!(scopes[0].name, "Color");

        // trait
        let source = "trait Drawable {\n    fn draw(&self);\n}\n";
        let scopes = scopes_at(source, 1);
        assert_eq!(scopes[0].kind, "trait_item");
        assert_eq!(scopes[0].name, "Drawable");

        // mod
        let source = "mod utils {\n    fn helper() {}\n}\n";
        let scopes = scopes_at(source, 1);
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
        let scopes = scopes_at(source, 3);
        assert_eq!(scopes[0].text, "impl Foo {");
        assert_eq!(scopes[1].text, "    fn compute(&self) -> i32 {");
    }

    #[test]
    fn expansion_anchor_grows_to_whole_wrapped_statement() {
        let source = "\
const proxy = createProxy(
  [\"A\", \"B\"],
  [\"C\"],
);
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // change on line 2 (the `[\"C\"]` argument), end-exclusive 3
        let anchor = parsed.expansion_anchor(2, 3, 200).expect("anchor");
        assert_eq!(anchor.kind, "lexical_declaration");
        assert_eq!(anchor.name, "");
        assert_eq!(anchor.start_line, 1);
        assert_eq!(anchor.end_line, 4);
    }

    #[test]
    fn expansion_anchor_excludes_root_and_picks_outermost_statement() {
        let source = "\
const a = 1;
const proxy = createProxy(
  [\"A\"],
  [\"C\"],
);
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // change on line 3 (`[\"C\"]`); anchor must be the proxy statement,
        // never the program root that also spans `const a = 1;`.
        let anchor = parsed.expansion_anchor(3, 4, 200).expect("anchor");
        assert_eq!(anchor.kind, "lexical_declaration");
        assert_eq!(anchor.start_line, 2);
        assert_eq!(anchor.end_line, 5);
    }

    #[test]
    fn expansion_anchor_floor_rejects_single_line_statement() {
        let source = "const x = 1;\nconst y = 2;\n";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // a 1-line statement is not bigger than the 1-line change
        assert!(parsed.expansion_anchor(0, 1, 200).is_none());
    }

    #[test]
    fn expansion_anchor_returns_none_when_only_too_large_container_exists() {
        let mut source = String::from("const proxy = createProxy(\n");
        for i in 0..220 {
            source.push_str(&format!("  [\"K{i}\"],\n"));
        }
        source.push_str(");\n");
        let parsed = ParsedFile::parse("app.ts", &source).expect("parse");
        // the statement is > 200 lines; the only smaller fitting ancestor
        // is a 1-line array == change size, rejected by the floor
        assert!(parsed.expansion_anchor(50, 51, 200).is_none());
    }

    #[test]
    fn expansion_anchor_includes_leading_comment_in_start() {
        let source = "\
// explain proxy
const proxy = createProxy(
  [\"A\"],
  [\"C\"],
);
";
        let parsed = ParsedFile::parse("app.ts", source).expect("parse");
        // change on line 3 (`[\"C\"]`); the anchor start extends up to the
        // leading `// explain proxy` comment on line 0.
        let anchor = parsed.expansion_anchor(3, 4, 200).expect("anchor");
        assert_eq!(anchor.kind, "lexical_declaration");
        assert_eq!(anchor.start_line, 1);
        assert_eq!(anchor.end_line, 5);
    }

    #[test]
    fn expansion_anchor_disabled_for_markdown() {
        let source = "\
# Title

Intro paragraph.

## Section

- bullet one
- bullet two
- bullet three
";
        let parsed = ParsedFile::parse("doc.md", source).expect("parse");
        // `- bullet two` (line 7) is body text with no structure ancestor.
        // Transparent expansion is off for prose, so no anchor is returned
        // and the change falls back to default context rather than growing
        // to the whole section/document.
        assert!(parsed.expansion_anchor(7, 8, 200).is_none());
    }

    #[test]
    fn enclosing_scopes_names_sql_create_table_from_object_reference() {
        let source = "\
CREATE TABLE public.users (
  id INT PRIMARY KEY,
  name TEXT
);
";
        let parsed = ParsedFile::parse("schema.sql", source).expect("parse");
        // line 2 (0-indexed) is the `name TEXT` column inside the table.
        let scope = parsed
            .enclosing_scopes(2)
            .into_iter()
            .find(|s| s.kind == "create_table")
            .expect("create_table scope");
        // The name comes from the nested `object_reference`, schema-qualified.
        assert_eq!(scope.name, "public.users");
    }

    #[test]
    fn enclosing_scopes_names_sql_create_function_body_edit() {
        let source = "\
CREATE FUNCTION add(a INT, b INT) RETURNS INT AS $$
BEGIN
  RETURN a + b;
END;
$$ LANGUAGE plpgsql;
";
        let parsed = ParsedFile::parse("fn.sql", source).expect("parse");
        // line 2 (0-indexed) is `RETURN a + b;` inside the function body.
        let scope = parsed
            .enclosing_scopes(2)
            .into_iter()
            .find(|s| s.kind == "create_function")
            .expect("create_function scope");
        assert_eq!(scope.name, "add");
    }

    #[test]
    fn enclosing_scopes_sql_comment_above_create_extends_start() {
        let source = "\
-- users table
CREATE TABLE users (
  id INT
);
";
        let parsed = ParsedFile::parse("schema.sql", source).expect("parse");
        // line 2 (0-indexed) is the `id INT` column.
        let scope = parsed
            .enclosing_scopes(2)
            .into_iter()
            .find(|s| s.kind == "create_table")
            .expect("create_table scope");
        // 1-indexed line 1 is the leading `-- users table` comment.
        assert_eq!(scope.start_line, 1);
        assert_eq!(scope.name, "users");
    }

    #[test]
    fn parses_kotlin_diff_cases_without_errors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diff_cases/cases");
        let paths: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name().to_str().unwrap().contains("-kotlin-"))
            .flat_map(|entry| std::fs::read_dir(entry.path()).unwrap().map(Result::unwrap))
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("kt" | "kts")
                )
            })
            .collect();
        assert!(!paths.is_empty());
        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            let parsed = ParsedFile::parse(path.to_str().unwrap(), &source).unwrap();
            assert!(!parsed.tree.root_node().has_error(), "{}", path.display());
        }
    }

    #[test]
    fn detects_all_supported_languages() {
        let cases = vec![
            ("test.rs", "fn main() {}"),
            ("test.sql", "SELECT 1;"),
            ("test.py", "def f(): pass"),
            ("test.js", "function f() {}"),
            ("test.ts", "function f() {}"),
            ("test.tsx", "function f() {}"),
            ("test.go", "package main"),
            ("test.rb", "def f; end"),
            ("test.java", "class A {}"),
            ("test.kt", "fun greet() {}"),
            ("test.kts", "println(\"Hello\")"),
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

    /// Breadcrumb names at the first line of `source` that contains
    /// `needle`, outermost first.
    fn names_at(path: &str, source: &str, needle: &str) -> Vec<String> {
        let line = source
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?}"));
        ParsedFile::parse(path, source)
            .expect("parse")
            .breadcrumb_scopes(line)
            .into_iter()
            .map(|scope| scope.name)
            .collect()
    }

    #[test]
    fn rust_impl_blocks_are_named_by_trait_and_type() {
        let source = "\
impl S {
    fn new() -> Self {
        S // inherent
    }
}

impl std::fmt::Display for S {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, \"s\") // trait
    }
}

impl<T: Clone> From<T> for Wrapper<T> {
    fn from(t: T) -> Self {
        Wrapper(t) // generic
    }
}
";
        assert_eq!(names_at("a.rs", source, "inherent"), ["S", "new"]);
        assert_eq!(
            names_at("a.rs", source, "// trait"),
            ["std::fmt::Display for S", "fmt"]
        );
        assert_eq!(
            names_at("a.rs", source, "generic"),
            ["From<T> for Wrapper<T>", "from"]
        );
    }

    #[test]
    fn go_methods_are_named_by_receiver_type() {
        let source = "\
package p

func (s *Server) Start() {
	s.n = 1 // pointer
}

func (c Client) Start() {
	c.n = 1 // value
}

func (l *List[T]) Push(v T) {
	l.n = 1 // generic
}
";
        assert_eq!(names_at("a.go", source, "pointer"), ["Server.Start"]);
        assert_eq!(names_at("a.go", source, "value"), ["Client.Start"]);
        assert_eq!(names_at("a.go", source, "generic"), ["List.Push"]);
    }

    #[test]
    fn go_type_declarations_are_named_by_the_types_they_declare() {
        let source = "\
package p

type Server struct {
	a int // single
}

type (
	A struct {
		x int // grouped
	}
	B = int
)
";
        assert_eq!(names_at("a.go", source, "single"), ["Server"]);
        assert_eq!(names_at("a.go", source, "grouped"), ["A, B"]);
    }

    #[test]
    fn ruby_singleton_methods_carry_their_receiver() {
        let source = "\
class C
  def self.build
    x = 1 # singleton
  end

  def build
    x = 1 # instance
  end
end
";
        assert_eq!(names_at("a.rb", source, "singleton"), ["C", "self.build"]);
        assert_eq!(names_at("a.rb", source, "instance"), ["C", "build"]);
    }

    #[test]
    fn c_functions_are_named_from_their_declarator() {
        let source = "\
int top(void) {
    return 1; /* plain */
}

char *name(void) {
    return 0; /* pointer */
}

static const char **names(int n) {
    return 0; /* double */
}

void (*handler(int sig))(int) {
    return 0; /* returns a function pointer */
}
";
        assert_eq!(names_at("a.c", source, "plain"), ["top"]);
        assert_eq!(names_at("a.c", source, "/* pointer"), ["name"]);
        assert_eq!(names_at("a.c", source, "double"), ["names"]);
        assert_eq!(names_at("a.c", source, "function pointer"), ["handler"]);
    }

    #[test]
    fn cpp_functions_are_named_from_their_declarator() {
        let source = "\
class Foo {
 public:
  int bar() {
    return 1; // inline
  }
};

int Foo::baz() {
  return 1; // qualified
}

Foo::~Foo() {
  x = 1; // destructor
}

bool Foo::operator==(const Foo& o) const {
  return true; // operator
}

int& Foo::ref() {
  return x; // reference
}

template <typename T>
T twice(T v) {
  return v; // template
}
";
        assert_eq!(names_at("a.cpp", source, "inline"), ["Foo", "bar"]);
        assert_eq!(names_at("a.cpp", source, "qualified"), ["Foo::baz"]);
        assert_eq!(names_at("a.cpp", source, "destructor"), ["Foo::~Foo"]);
        assert_eq!(
            names_at("a.cpp", source, "// operator"),
            ["Foo::operator=="]
        );
        assert_eq!(names_at("a.cpp", source, "reference"), ["Foo::ref"]);
        assert_eq!(names_at("a.cpp", source, "// template"), ["twice"]);
    }

    #[test]
    fn css_rules_are_named_by_selectors_and_at_rules_by_their_header() {
        let source = "\
.a, .b > p {
  color: red; /* rule */
}

@media (max-width: 600px) {
  .c {
    color: red; /* nested */
  }
}
";
        assert_eq!(names_at("a.css", source, "rule"), [".a, .b > p"]);
        assert_eq!(
            names_at("a.css", source, "nested"),
            ["@media (max-width: 600px)", ".c"]
        );
    }

    #[test]
    fn toml_tables_are_named_by_their_key() {
        let source = "\
[package]
name = \"x\" # bare

[dependencies.serde]
version = \"1\" # dotted

[[bin]]
name = \"a\" # array

[\"quoted key\"]
a = 1 # quoted
";
        assert_eq!(names_at("a.toml", source, "bare"), ["package"]);
        assert_eq!(names_at("a.toml", source, "dotted"), ["dependencies.serde"]);
        assert_eq!(names_at("a.toml", source, "# array"), ["bin"]);
        assert_eq!(names_at("a.toml", source, "# quoted"), ["\"quoted key\""]);
    }

    #[test]
    fn markdown_headings_are_named_by_their_text() {
        let source = "\
# Title

Intro.

## Panels and tabs

Body.

Setext heading
==============
";
        assert_eq!(names_at("a.md", source, "## Panels"), ["Panels and tabs"]);
        assert_eq!(
            names_at("a.md", source, "Setext heading"),
            ["Setext heading"]
        );
    }
}
