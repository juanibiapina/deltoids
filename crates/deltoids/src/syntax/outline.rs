//! A parsed file's definitions, nested as written, with hashes of each
//! one's own text so two versions of a file can be compared.
//!
//! [`ParsedFile::outline`] walks the tree twice. The first walk finds the
//! definitions top-down, with the same scope and nesting rules as the
//! breadcrumbs (a scope nested in a function body is part of that body)
//! plus the language's leaf definitions. The second walk hands every token
//! to the innermost definition whose span holds it, sorted into its
//! signature, body, comments, or the docs above it.

use std::hash::{Hash, Hasher};
use std::ops::Range;

use rustc_hash::FxHasher;
use tree_sitter::Node;

use super::visibility::{self, Declared};
use super::{Field, ParsedFile, collapse_whitespace, is_inner_doc_comment, node_end_line};
use crate::definitions::{DefinitionKind, Visibility};
use crate::language::{LeafDefinition, Require};

/// The definitions of one version of a file.
pub(crate) struct Outline {
    /// In document order; a definition comes before the ones it holds.
    pub(crate) definitions: Vec<Definition>,
    /// The tokens outside every definition, in document order.
    pub(crate) top: Vec<TopToken>,
    pub(crate) has_errors: bool,
}

/// A token outside every definition.
pub(crate) struct TopToken {
    /// 0-based rows it covers, end exclusive.
    pub(crate) rows: Range<usize>,
    pub(crate) stream: TopStream,
    pub(crate) hash: u64,
}

/// What a token outside every definition is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopStream {
    Comments,
    Imports,
    Code,
}

pub(crate) struct Definition {
    pub(crate) parent: Option<usize>,
    pub(crate) kind: DefinitionKind,
    pub(crate) node_kind: &'static str,
    /// A scope (it names breadcrumbs), not a leaf definition.
    #[cfg(test)]
    scope: bool,
    pub(crate) name: String,
    /// Bytes from the comments and attributes above it to its end.
    pub(crate) span: Range<usize>,
    /// 0-based rows of `span`, end exclusive.
    pub(crate) rows: Range<usize>,
    /// 0-based row its own syntax starts on, after the docs above it.
    pub(crate) head_row: usize,
    /// Its header on one line.
    pub(crate) signature: String,
    pub(crate) content: Content,
    /// It holds a syntax error.
    pub(crate) error: bool,
    /// Who can use it; see `syntax::visibility`.
    pub(crate) visibility: Option<Visibility>,
    /// Where its own syntax starts: after the comments and attributes
    /// above it, at any wrapper that starts on its line.
    head: usize,
    /// Where its node starts.
    #[cfg(test)]
    node_start: usize,
    /// Where the comments its node ends with start, when nothing but
    /// comments follows its last token (a TOML table's trailing comments
    /// document the next table).
    tail: Option<usize>,
    body: Option<usize>,
    name_bytes: Option<Range<usize>>,
}

/// Hashes of a definition's own text, nested definitions left out.
#[derive(Debug, Default, Clone)]
pub(crate) struct Content {
    pub(crate) signature: u64,
    /// The signature without the definition's name token.
    pub(crate) unnamed_signature: u64,
    pub(crate) body: u64,
    pub(crate) comments: u64,
    pub(crate) docs: u64,
    /// The attributes and decorators above it.
    pub(crate) attributes: u64,
    /// Every byte, whitespace included.
    pub(crate) raw: u64,
    /// Token hashes of the unnamed signature and the body, for similarity.
    pub(crate) tokens: Vec<u64>,
}

impl ParsedFile {
    /// The file's definitions.
    pub(crate) fn outline(&self) -> Outline {
        let mut builder = Builder {
            parsed: self,
            roles: Roles::new(&self.config, &self.tree.language()),
            definitions: Vec::new(),
            declared: Vec::new(),
            last_child: vec![None],
        };
        let tokens = builder.find_definitions();
        let (streams, top, has_errors) = builder.assign_tokens(&tokens);
        let mut definitions = builder.definitions;
        for (definition, streams) in definitions.iter_mut().zip(streams) {
            definition.content = streams.finish();
        }
        raw_hashes(&self.source, &mut definitions);
        visibility::resolve(&mut definitions, &builder.declared, self.exports());
        Outline {
            definitions,
            top,
            has_errors,
        }
    }
}

struct Builder<'a> {
    parsed: &'a ParsedFile,
    roles: Roles,
    definitions: Vec<Definition>,
    /// What each definition's syntax says about who can use it.
    declared: Vec<Declared>,
    /// The last definition found in each definition, by index, and in the
    /// file (the last slot, kept one past the definitions).
    last_child: Vec<Option<usize>>,
}

/// What the first walk knows about a node on its path. Tree-sitter finds
/// a node's parent and siblings by scanning, so the walk keeps them.
struct Frame<'t> {
    node: Node<'t>,
    /// Its children sit in a function body (see
    /// `ParsedFile::function_scope`).
    in_function: bool,
    /// The nearest definition holding its children.
    container: Option<usize>,
    /// Where the kinds between `container` and its children start in the
    /// walk's path.
    base: usize,
    /// It pushed its kind onto the path.
    pushed: bool,
    /// Where the comments and decorators right above it start.
    leading: Option<usize>,
    /// Among its children so far: where the run of comments and
    /// decorators, each starting its line, that the next child follows
    /// starts.
    run: Option<usize>,
    /// The definition its last named child holds, for a `;` after it.
    previous: Option<usize>,
    /// What it and its ancestors say about its tokens.
    marks: Marks,
}

impl Builder<'_> {
    /// Find the definitions, and list every token for
    /// [`Builder::assign_tokens`].
    fn find_definitions(&mut self) -> Vec<Token> {
        let root = self.parsed.tree.root_node();
        let mut finder = Finder {
            builder: self,
            frames: vec![Frame {
                node: root,
                in_function: false,
                container: None,
                base: 0,
                pushed: false,
                leading: None,
                run: None,
                previous: None,
                marks: Marks {
                    error: root.is_error(),
                    ..Marks::default()
                },
            }],
            path: Vec::new(),
            tokens: Vec::new(),
        };
        walk(root, &mut finder);
        finder.tokens
    }

    fn enter<'t>(
        &mut self,
        node: Node<'t>,
        role: Role,
        marks: Marks,
        frames: &mut Vec<Frame<'t>>,
        path: &mut Vec<u16>,
    ) {
        let leading = self.follow_siblings(node, role, frames);
        let parent = frames.last().expect("the root frame");
        let (in_function, container, base) = (parent.in_function, parent.container, parent.base);
        let children_in_function = self.function_scope(node, role).unwrap_or(in_function);
        let defined = role
            .named
            .then(|| self.definition_kind(node, role, in_function, container, &path[base..]))
            .flatten()
            .and_then(|kind| self.push(node, kind, container, leading, frames));
        let pushed = defined.is_none();
        if pushed {
            path.push(node.kind_id());
        }
        frames.push(Frame {
            node,
            in_function: children_in_function,
            container: defined.or(container),
            base: if pushed { base } else { path.len() },
            pushed,
            leading,
            run: None,
            previous: None,
            marks,
        });
    }

    /// Note `node` among its siblings: where the comments right above it
    /// start, and the run of comments it continues or ends. A `;` right
    /// after a definition on its last line joins it.
    fn follow_siblings(&mut self, node: Node, role: Role, frames: &mut [Frame]) -> Option<usize> {
        let parsed = self.parsed;
        let parent = frames.last_mut().expect("the root frame");
        if !role.named {
            let previous = parent.previous.take();
            if let Some(index) = previous.filter(|_| node.kind() == ";") {
                self.adopt_semicolon(index, node);
            }
            return None;
        }
        parent.previous = None;
        if !role.leading || is_inner_doc_comment(node) {
            return parent.run.take();
        }
        let line = parsed.lines.get(node.start_position().row);
        let starts_line = line.is_some_and(|line| {
            parsed.source[line.start..node.start_byte()]
                .trim()
                .is_empty()
        });
        parent.run = match (starts_line, line) {
            (true, Some(line)) => parent.run.or(Some(line.start)),
            _ => None,
        };
        None
    }

    /// Extend definition `index` over the `;` after it, when only spaces
    /// sit between them on one line.
    fn adopt_semicolon(&mut self, index: usize, semicolon: Node) {
        let parsed = self.parsed;
        let definition = &mut self.definitions[index];
        let end = definition.span.end;
        let same_line = row_of(&parsed.lines, end) == semicolon.start_position().row;
        let gap = parsed
            .source
            .get(end..semicolon.start_byte())
            .unwrap_or("x");
        if same_line && gap.trim().is_empty() {
            definition.span.end = semicolon.end_byte();
        }
    }

    /// The kind of definition `node` is, when it is one. `path` holds the
    /// kinds between it and `container`. Follows the breadcrumb rules (see
    /// `ParsedFile::enclosing_scope_nodes`) through the kind table.
    fn definition_kind(
        &self,
        node: Node,
        role: Role,
        in_function: bool,
        container: Option<usize>,
        path: &[u16],
    ) -> Option<DefinitionKind> {
        let parsed = self.parsed;
        let labeled = || role.call_promoted && parsed.is_labeled_call_promoted(node);
        let nested = || in_function && !labeled();
        let is_scope = role.structure
            || role.anchor_only
            || (role.promoted && parsed.has_function_value(&node))
            || (role.call_promoted && parsed.has_function_argument(node));
        let category = if is_scope {
            let block_only = role.call_promoted && parsed.is_block_only_call(node);
            if nested() || role.anchor_only || block_only {
                return None;
            }
            role.category?
        } else {
            let leaf = &parsed.config.leaf_definitions[usize::from(role.leaf?)];
            if nested() || !self.leaf_applies(leaf, node, container, path) {
                return None;
            }
            leaf.category
        };
        let holds_methods =
            container.is_some_and(|index| self.definitions[index].kind.holds_methods());
        Some(match category {
            DefinitionKind::Function if holds_methods => DefinitionKind::Method,
            other => other,
        })
    }

    /// [`ParsedFile::function_scope`] through the kind table.
    fn function_scope(&self, node: Node, role: Role) -> Option<bool> {
        if role.function_body && !role.anchor_only {
            return Some(true);
        }
        if role.call_promoted && self.parsed.is_labeled_call_promoted(node) {
            return Some(false);
        }
        None
    }

    fn leaf_applies(
        &self,
        leaf: &LeafDefinition,
        node: Node,
        container: Option<usize>,
        path: &[u16],
    ) -> bool {
        let language = node.language();
        let through = |&id: &u16| {
            language
                .node_kind_for_id(id)
                .is_some_and(|kind| leaf.through.contains(&kind))
        };
        if !path.iter().all(through) {
            return false;
        }
        if let Some(levels) = leaf.levels {
            let same = std::iter::successors(container, |&index| self.definitions[index].parent)
                .filter(|&index| self.definitions[index].node_kind == leaf.kind)
                .count();
            if same > levels {
                return false;
            }
        }
        let parsed = self.parsed;
        match leaf.require {
            Require::Nothing => true,
            Require::Body => parsed.child(node, Field::Body).is_some(),
            Require::FunctionDeclarator => {
                declarators(parsed, node).any(|d| d.kind() == "function_declarator")
            }
            Require::Left(kinds) => parsed
                .child(node, Field::Left)
                .is_some_and(|left| kinds.contains(&left.kind())),
            Require::FunctionValue => node
                .named_child((node.named_child_count() as u32).saturating_sub(1))
                .and_then(|values| values.named_child(0))
                .is_some_and(|value| parsed.config.function_body_kinds.contains(&value.kind())),
        }
    }

    /// Record `node` as a definition; `None` when it has no name.
    /// `leading` is where the comments right above it start; `frames`
    /// hold its ancestors, the file root first.
    fn push<'t>(
        &mut self,
        node: Node<'t>,
        kind: DefinitionKind,
        container: Option<usize>,
        leading: Option<usize>,
        frames: &mut [Frame<'t>],
    ) -> Option<usize> {
        let parsed = self.parsed;
        let name = parsed.scope_name(node);
        if name.is_empty() {
            return None;
        }
        let (wrapper, wrapper_depth) = same_line_wrapper(node, frames);
        let leading = same_row_leading(node, leading, frames);
        // Comments above it may share a line with the definition before it
        // or with its container's header; the span never reaches into them.
        let floor = self
            .previous_sibling(container)
            .map(|previous| self.definitions[previous].span.end)
            .into_iter()
            .chain(container.map(|index| self.definitions[index].head))
            .max()
            .unwrap_or(0);
        let mut start = leading
            .unwrap_or(wrapper.start_byte())
            .max(floor)
            .min(wrapper.start_byte());
        if leading.is_none() {
            self.adopt_trailing_comments(container, wrapper.start_byte(), &mut start);
        }
        let end = wrapper.end_byte();
        let rows = row_of(&parsed.lines, start)..node_end_line(wrapper);
        let head = wrapper.start_byte();
        let body = parsed.body_start(node, &self.roles);
        let signature = parsed.signature_text(head, body, end);
        self.definitions.push(Definition {
            parent: container,
            kind,
            node_kind: node.kind(),
            #[cfg(test)]
            scope: parsed.is_scope_kind(node),
            name,
            span: start..end,
            rows,
            head_row: row_of(&parsed.lines, head),
            signature,
            content: Content::default(),
            error: false,
            visibility: None,
            head,
            #[cfg(test)]
            node_start: node.start_byte(),
            tail: trailing_comments(node, &self.roles),
            body,
            name_bytes: parsed.name_node(node).map(|name| name.byte_range()),
        });
        let ancestors: Vec<Node> = frames
            .iter()
            .rev()
            .take_while(|frame| frame.container == container)
            .map(|frame| frame.node)
            .collect();
        let container_kind = container.map(|index| self.definitions[index].kind);
        let name = &self.definitions[self.definitions.len() - 1].name;
        let declared = parsed.declared_visibility(node, kind, name, container_kind, &ancestors);
        self.declared.push(declared);
        let index = self.definitions.len() - 1;
        let root = self.last_child.len() - 1;
        self.last_child[container.unwrap_or(root)] = Some(index);
        // Keep the file's slot last.
        self.last_child.insert(root, None);
        // A `;` after the wrapper, among the wrapper's siblings, joins it.
        if let Some(frame) = wrapper_depth
            .checked_sub(1)
            .and_then(|depth| frames.get_mut(depth))
        {
            frame.previous = Some(index);
        }
        Some(index)
    }

    /// The last definition found so far in `container` (the file for
    /// `None`).
    fn previous_sibling(&self, container: Option<usize>) -> Option<usize> {
        let root = self.last_child.len() - 1;
        self.last_child[container.unwrap_or(root)]
    }

    /// Take the comments the previous sibling definition ends with, when
    /// only whitespace separates them from this definition at `head`:
    /// they document this one. Moves `start` up to them and ends the
    /// previous definition before them.
    fn adopt_trailing_comments(
        &mut self,
        container: Option<usize>,
        head: usize,
        start: &mut usize,
    ) {
        let parsed = self.parsed;
        let Some(previous) = self.previous_sibling(container) else {
            return;
        };
        let previous = &mut self.definitions[previous];
        let Some(tail) = previous.tail else {
            return;
        };
        let between = parsed.source.get(previous.span.end..head).unwrap_or("x");
        if !between.trim().is_empty() {
            return;
        }
        let row = row_of(&parsed.lines, tail);
        let line_start = parsed.lines.get(row).map_or(tail, |line| line.start);
        previous.span.end = line_start.max(previous.head);
        previous.rows.end = row.max(previous.rows.start + 1);
        previous.tail = None;
        *start = line_start.min(*start);
    }

    /// Hand every token to the definition that owns it, with the text
    /// between tokens that is not whitespace (a grammar may leave it to no
    /// token) counted as a token of its own.
    fn assign_tokens(&mut self, tokens: &[Token]) -> (Vec<Streams>, Vec<TopToken>, bool) {
        let mut assign = Assignment {
            streams: self
                .definitions
                .iter()
                .map(|_| Streams::default())
                .collect(),
            top: Vec::new(),
            has_errors: false,
            open: Vec::new(),
            next: 0,
            last_end: 0,
        };
        for token in tokens {
            assign.gap(self, token.start, token.marks.import);
            assign.last_end = assign.last_end.max(token.end);
            let owner = self.owner(token.start, &mut assign.open, &mut assign.next);
            if token.marks.error || token.missing {
                assign.has_errors = true;
                owner.inspect(|&index| self.definitions[index].error = true);
            }
            if !token.missing {
                assign.add(self, owner, token.start..token.end, token.hash, token.marks);
            }
        }
        assign.gap(self, self.parsed.source.len(), false);
        (assign.streams, assign.top, assign.has_errors)
    }

    /// The innermost definition whose span holds byte `at`. Calls come in
    /// document order: `open` holds the definitions open at the last
    /// call, `next` the first not yet reached.
    fn owner(&self, at: usize, open: &mut Vec<usize>, next: &mut usize) -> Option<usize> {
        while open
            .last()
            .is_some_and(|&index| self.definitions[index].span.end <= at)
        {
            open.pop();
        }
        while *next < self.definitions.len() && self.definitions[*next].span.start <= at {
            let parent = self.definitions[*next].parent;
            while !open.is_empty() && open.last().copied() != parent {
                open.pop();
            }
            open.push(*next);
            *next += 1;
            while open
                .last()
                .is_some_and(|&index| self.definitions[index].span.end <= at)
            {
                open.pop();
            }
        }
        open.last().copied()
    }

    /// A token outside every definition at `bytes`: the rows it covers,
    /// and whether it is a comment, an import, or code.
    fn top_token(&self, bytes: Range<usize>, hash: u64, marks: Marks) -> TopToken {
        let lines = &self.parsed.lines;
        let first = row_of(lines, bytes.start);
        let last = row_of(lines, bytes.end.saturating_sub(1).max(bytes.start));
        let stream = if marks.comment {
            TopStream::Comments
        } else if marks.import {
            TopStream::Imports
        } else {
            TopStream::Code
        };
        TopToken {
            rows: first..last.max(first) + 1,
            stream,
            hash,
        }
    }

    /// Add `token` at byte `at` to the stream of definition `index` it
    /// belongs in: the docs above it, its comments, signature, or body.
    fn route(&self, index: usize, at: usize, token: u64, marks: Marks, streams: &mut [Streams]) {
        let comment = marks.comment;
        let definition = &self.definitions[index];
        let stream = &mut streams[index];
        if comment {
            if at < definition.head {
                token.hash(&mut stream.docs);
            } else {
                token.hash(&mut stream.comments);
            }
        } else if at < definition.head {
            token.hash(&mut stream.attributes);
        } else if definition.body.is_some_and(|body| at >= body) {
            token.hash(&mut stream.body);
            stream.tokens.push(token);
        } else {
            token.hash(&mut stream.signature);
            let is_name = definition
                .name_bytes
                .as_ref()
                .is_some_and(|name| name.contains(&at));
            if !is_name {
                token.hash(&mut stream.unnamed_signature);
                stream.tokens.push(token);
            }
        }
    }
}

/// What each node kind is to the outline, by kind id: the language's
/// lists, looked up once instead of compared by name at every node.
struct Roles(Vec<Role>);

#[derive(Debug, Default, Clone, Copy)]
struct Role {
    named: bool,
    /// A kind a definition's body starts at (see `body_kinds`).
    body: bool,
    structure: bool,
    promoted: bool,
    anchor_only: bool,
    call_promoted: bool,
    function_body: bool,
    comment: bool,
    /// A comment, attribute, or decorator that may sit above a definition
    /// (Rust inner doc comments excepted, per node).
    leading: bool,
    import: bool,
    category: Option<DefinitionKind>,
    /// Index of its rule in `leaf_definitions`.
    leaf: Option<u8>,
}

impl Roles {
    fn new(config: &crate::language::TreeSitterConfig, language: &tree_sitter::Language) -> Self {
        let roles = (0..language.node_kind_count())
            .map(|id| {
                let Some(kind) = u16::try_from(id)
                    .ok()
                    .and_then(|id| language.node_kind_for_id(id))
                else {
                    return Role::default();
                };
                Role {
                    named: u16::try_from(id).is_ok_and(|id| language.node_kind_is_named(id)),
                    body: config.body_kinds.contains(&kind),
                    structure: config.structure_kinds.contains(&kind),
                    promoted: config.promoted_kinds.contains(&kind),
                    anchor_only: config.anchor_only_kinds.contains(&kind),
                    call_promoted: config.call_promoted_kinds.contains(&kind),
                    function_body: config.function_body_kinds.contains(&kind),
                    comment: kind.contains("comment"),
                    leading: kind == "decorator" || config.leading_comment_kinds.contains(&kind),
                    import: config.import_kinds.contains(&kind),
                    category: config
                        .definition_kinds
                        .iter()
                        .find(|(definition, _)| *definition == kind)
                        .map(|(_, category)| *category),
                    leaf: config
                        .leaf_definitions
                        .iter()
                        .position(|leaf| leaf.kind == kind)
                        .and_then(|index| u8::try_from(index).ok()),
                }
            })
            .collect();
        Roles(roles)
    }

    fn of(&self, node: Node) -> Role {
        self.get(node.kind_id())
    }

    fn get(&self, kind_id: u16) -> Role {
        self.0
            .get(usize::from(kind_id))
            .copied()
            .unwrap_or_default()
    }
}

/// What a walk does at each node.
trait Visit<'t> {
    /// On the way down; `false` skips the node's children.
    fn enter(&mut self, node: Node<'t>) -> bool;
    /// On the way back up.
    fn leave(&mut self, node: Node<'t>);
}

/// Visit `root`'s descendants in document order.
fn walk<'t>(root: Node<'t>, visitor: &mut impl Visit<'t>) {
    let mut cursor = root.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        if visitor.enter(cursor.node()) && cursor.goto_first_child() {
            continue;
        }
        loop {
            visitor.leave(cursor.node());
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() || cursor.node().id() == root.id() {
                return;
            }
        }
    }
}

/// Where [`Builder::assign_tokens`] stands.
struct Assignment {
    streams: Vec<Streams>,
    top: Vec<TopToken>,
    has_errors: bool,
    /// See [`Builder::owner`].
    open: Vec<usize>,
    next: usize,
    /// End of the last token.
    last_end: usize,
}

impl Assignment {
    /// Hand the token at `bytes` to definition `owner`'s streams, or to
    /// the file's tokens outside every definition.
    fn add(
        &mut self,
        builder: &Builder,
        owner: Option<usize>,
        bytes: Range<usize>,
        hash: u64,
        marks: Marks,
    ) {
        match owner {
            Some(index) => builder.route(index, bytes.start, hash, marks, &mut self.streams),
            None => self.top.push(builder.top_token(bytes, hash, marks)),
        }
    }

    /// Count the text from the last token to `until` as a token when it is
    /// not whitespace.
    fn gap(&mut self, builder: &Builder, until: usize, import: bool) {
        let source = builder.parsed.source.as_str();
        let text = source.get(self.last_end.min(until)..until).unwrap_or("");
        if text.trim().is_empty() {
            return;
        }
        let at = self.last_end + (text.len() - text.trim_start().len());
        let hash = token_hash(u16::MAX, text.trim().as_bytes());
        let owner = builder.owner(at, &mut self.open, &mut self.next);
        let marks = Marks {
            import,
            ..Marks::default()
        };
        self.add(builder, owner, at..at + text.trim().len(), hash, marks);
    }
}

/// The walk: finds the definitions and lists the tokens.
struct Finder<'b, 'a> {
    builder: &'b mut Builder<'a>,
    frames: Vec<Frame<'a>>,
    /// Kind ids between the nearest definition and the node.
    path: Vec<u16>,
    tokens: Vec<Token>,
}

/// A token: its bytes, their hash, and what the nodes around it say.
struct Token {
    start: usize,
    end: usize,
    hash: u64,
    marks: Marks,
    missing: bool,
}

impl<'a> Visit<'a> for Finder<'_, 'a> {
    fn enter(&mut self, node: Node<'a>) -> bool {
        let kind_id = node.kind_id();
        let role = self.builder.roles.get(kind_id);
        let parent = self
            .frames
            .last()
            .map(|frame| frame.marks)
            .unwrap_or_default();
        let marks = Marks {
            comment: parent.comment || role.comment,
            import: parent.import || role.import,
            // Tree-sitter's ERROR kind.
            error: parent.error || kind_id == u16::MAX,
        };
        self.builder
            .enter(node, role, marks, &mut self.frames, &mut self.path);
        // A comment is one token: some grammars give a comment's text to
        // no child.
        if role.comment || node.child_count() == 0 {
            let source = self.builder.parsed.source.as_bytes();
            self.tokens.push(Token {
                start: node.start_byte(),
                end: node.end_byte(),
                hash: token_hash(node.kind_id(), &source[node.byte_range()]),
                marks,
                missing: node.is_missing(),
            });
            return false;
        }
        true
    }

    fn leave(&mut self, _: Node<'a>) {
        if self.frames.pop().is_some_and(|frame| frame.pushed) {
            self.path.pop();
        }
    }
}

/// What the nodes around a token say about it.
#[derive(Debug, Default, Clone, Copy)]
struct Marks {
    comment: bool,
    import: bool,
    error: bool,
}

/// `node`'s declarator chain, outermost first: down the `declarator`
/// fields (and the plain child of reference and parenthesized
/// declarators) to the first node that is not a declarator.
fn declarators<'a>(parsed: &ParsedFile, node: Node<'a>) -> impl Iterator<Item = Node<'a>> {
    std::iter::successors(parsed.child(node, Field::Declarator), move |declarator| {
        if !declarator.kind().ends_with("declarator") {
            return None;
        }
        parsed
            .child(*declarator, Field::Declarator)
            .or_else(|| declarator.named_child(0))
    })
}

impl ParsedFile {
    /// Where `node`'s body starts: its `body`, the first child of a body
    /// kind, the body of the function its call or value holds, its
    /// value, what follows its `=`, or its second line. `None` for a
    /// one-line definition with none of those.
    fn body_start(&self, node: Node, roles: &Roles) -> Option<usize> {
        if let Some(body) = self.child(node, Field::Body) {
            return Some(body.start_byte());
        }
        let mut cursor = node.walk();
        if let Some(body) = node
            .children(&mut cursor)
            .find(|child| roles.of(*child).body)
        {
            return Some(body.start_byte());
        }
        if self.config.call_promoted_kinds.contains(&node.kind()) {
            let function = self.child(node, Field::Block).or_else(|| {
                let arguments = self.child(node, Field::Arguments)?;
                let mut cursor = arguments.walk();
                arguments
                    .named_children(&mut cursor)
                    .find(|argument| self.config.function_body_kinds.contains(&argument.kind()))
            });
            if let Some(function) = function {
                let body = self.child(function, Field::Body).unwrap_or(function);
                return Some(body.start_byte());
            }
        }
        let value = self
            .child(node, Field::Value)
            .or_else(|| self.child(node, Field::Right))
            .or_else(|| {
                self.child(node, Field::Declarator)
                    .and_then(|declarator| self.child(declarator, Field::Value))
            });
        if let Some(value) = value {
            let body = self.child(value, Field::Body).unwrap_or(value);
            return Some(body.start_byte());
        }
        let mut cursor = node.walk();
        if let Some(equals) = node.children(&mut cursor).find(|child| child.kind() == "=") {
            return Some(equals.end_byte());
        }
        let row = node.start_position().row;
        (node.end_position().row > row)
            .then(|| self.lines.get(row + 1).map(|line| line.start))
            .flatten()
            .filter(|&start| start < node.end_byte())
    }

    /// The token that names `node`, when one does: its `name` field, or
    /// the end of a C declarator chain.
    pub(super) fn name_node<'a>(&self, node: Node<'a>) -> Option<Node<'a>> {
        if let Some(name) = self.child(node, Field::Name) {
            return Some(name);
        }
        let mut current = self.child(node, Field::Declarator);
        while let Some(declarator) = current {
            if !declarator.kind().ends_with("declarator") {
                return Some(declarator);
            }
            current = self
                .child(declarator, Field::Declarator)
                .or_else(|| declarator.named_child(0));
        }
        None
    }

    /// The header from `head` to `body` on one line, without the `=`
    /// before a value; a definition without a body shows its first line.
    pub(super) fn signature_text(&self, head: usize, body: Option<usize>, end: usize) -> String {
        let end = match body {
            Some(body) => body.min(end),
            None => {
                let row = row_of(&self.lines, head);
                self.lines
                    .get(row)
                    .map_or(end, |line| line.end.min(end).max(head))
            }
        };
        without_assignment(collapse_whitespace(&self.source[head..end]))
    }
}

/// `text` without the `=` that ends it before a value: `const LIMIT: u32`
/// for `const LIMIT: u32 = 10;`. An `=` that ends an operator (Ruby's
/// `def ==`, `def <=`) stays.
fn without_assignment(text: String) -> String {
    match text.strip_suffix('=') {
        Some(rest) if !rest.ends_with(|c: char| "=!<>+-*/%&|^~:".contains(c)) => {
            rest.trim_end().to_string()
        }
        _ => text,
    }
}

/// The node plus wrappers that start on its line and end where it ends
/// (an `export`, a `const` around one declarator, a trailing `;`), below
/// the file root; and the wrapper's depth among `frames` (the depth
/// `node`'s own frame will have when it is the wrapper). `frames` hold
/// `node`'s ancestors, the root first.
fn same_line_wrapper<'t>(node: Node<'t>, frames: &[Frame<'t>]) -> (Node<'t>, usize) {
    let row = node.start_position().row;
    let mut wrapper = (node, frames.len());
    for depth in (1..frames.len()).rev() {
        let parent = frames[depth].node;
        if parent.start_position().row != row
            || parent.end_byte() > node.end_byte() + 1
            || parent.named_child_count() != 1
        {
            break;
        }
        wrapper = (parent, depth);
    }
    wrapper
}

/// Where the comments above `node`'s outermost ancestor that starts on its
/// line, below the root, start: `leading` is `node`'s own. Comments above
/// `export const f` sit above the `export`.
fn same_row_leading(node: Node, leading: Option<usize>, frames: &[Frame]) -> Option<usize> {
    let row = node.start_position().row;
    let mut found = leading;
    for frame in frames.iter().skip(1).rev() {
        if frame.node.start_position().row != row {
            break;
        }
        found = frame.leading;
    }
    found
}

/// Where the comments `node` ends with start, when its last children are
/// comments.
fn trailing_comments(node: Node<'_>, roles: &Roles) -> Option<usize> {
    let mut cursor = node.walk();
    let mut tail = None;
    for child in node.children(&mut cursor) {
        let comment = roles.of(child).leading && !is_inner_doc_comment(child);
        tail = match (comment, tail) {
            (true, None) => Some(child.start_byte()),
            (true, kept) => kept,
            (false, _) => None,
        };
    }
    tail
}

/// The 0-based row holding byte `at`.
fn row_of(lines: &[Range<usize>], at: usize) -> usize {
    lines
        .partition_point(|line| line.end < at)
        .min(lines.len().saturating_sub(1))
}

fn token_hash(kind: u16, bytes: &[u8]) -> u64 {
    let mut hasher = FxHasher::default();
    kind.hash(&mut hasher);
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[derive(Default)]
struct Streams {
    signature: FxHasher,
    unnamed_signature: FxHasher,
    body: FxHasher,
    comments: FxHasher,
    docs: FxHasher,
    attributes: FxHasher,
    tokens: Vec<u64>,
}

impl Streams {
    fn finish(self) -> Content {
        Content {
            signature: self.signature.finish(),
            unnamed_signature: self.unnamed_signature.finish(),
            body: self.body.finish(),
            comments: self.comments.finish(),
            docs: self.docs.finish(),
            attributes: self.attributes.finish(),
            raw: 0,
            tokens: self.tokens,
        }
    }
}

/// Hash each definition's own text: the pieces between nested
/// definitions, each trimmed, so adding or removing a nested definition
/// leaves its parent's hash alone.
fn raw_hashes(source: &str, definitions: &mut [Definition]) {
    let own = |span: Range<usize>, holes: &mut dyn Iterator<Item = Range<usize>>| {
        let mut hasher = FxHasher::default();
        let mut piece = |range: Range<usize>| {
            let text = source.get(range).unwrap_or("").trim();
            if !text.is_empty() {
                hasher.write(text.as_bytes());
                hasher.write_u8(0xff);
            }
        };
        let mut at = span.start;
        for hole in holes {
            piece(at..hole.start.max(at));
            at = at.max(hole.end);
        }
        piece(at..span.end.max(at));
        hasher.finish()
    };
    // Children of each definition, in document order.
    let mut children: Vec<Vec<Range<usize>>> = vec![Vec::new(); definitions.len()];
    for definition in definitions.iter() {
        if let Some(parent) = definition.parent {
            children[parent].push(definition.span.clone());
        }
    }
    for (index, definition) in definitions.iter_mut().enumerate() {
        definition.content.raw = own(
            definition.span.clone(),
            &mut children[index].iter().cloned(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line per definition: two spaces per level of nesting, its kind,
    /// and its name.
    fn outline(path: &str, source: &str) -> Vec<String> {
        let parsed = ParsedFile::parse(path, source).expect("parse");
        let outline = parsed.outline();
        outline
            .definitions
            .iter()
            .map(|definition| {
                let mut depth = 0;
                let mut parent = definition.parent;
                while let Some(index) = parent {
                    depth += 1;
                    parent = outline.definitions[index].parent;
                }
                format!(
                    "{}{} {}",
                    "  ".repeat(depth),
                    definition.kind.label(),
                    definition.name
                )
            })
            .collect()
    }

    /// Each definition's signature, as the definitions view shows it.
    fn signatures(path: &str, source: &str) -> Vec<String> {
        let parsed = ParsedFile::parse(path, source).expect("parse");
        parsed
            .outline()
            .definitions
            .into_iter()
            .map(|definition| definition.signature)
            .collect()
    }

    #[test]
    fn a_signature_ends_before_the_equals_of_a_value() {
        assert_eq!(
            signatures(
                "a.rs",
                "const LIMIT: u32 = 10;\nstatic NAME: &str = \"x\";\n"
            ),
            ["const LIMIT: u32", "static NAME: &str"]
        );
        assert_eq!(signatures("a.py", "LIMIT = 10\n"), ["LIMIT"]);
        assert_eq!(
            signatures("a.rb", "class C\n  def ==\n    true\n  end\nend\n"),
            ["class C", "def =="]
        );
    }

    #[test]
    fn rust_items_nest_in_impls_traits_and_modules() {
        let source = "\
use std::fmt;

/// Doc
#[derive(Debug)]
pub struct S {
    a: u8,
}

const C: u8 = 1;
static G: u8 = 2;
type Alias = u8;
macro_rules! m { () => {} }

trait T {
    fn required(&self);
    fn provided(&self) {}
}

impl S {
    pub fn new() -> Self {
        fn helper() {}
        const LOCAL: u8 = 3;
        S { a: 1 }
    }
}

mod inner {
    pub fn f() {}
}
";
        assert_eq!(
            outline("a.rs", source),
            [
                "struct S",
                "constant C",
                "constant G",
                "type alias Alias",
                "macro m",
                "trait T",
                "  method required",
                "  method provided",
                "impl S",
                "  method new",
                "module inner",
                "  function f",
            ]
        );
    }

    #[test]
    fn python_lists_module_and_class_variables_but_not_locals() {
        let source = "\
import os

X = 1

class C:
    y = 2

    def m(self):
        z = 3

def f():
    pass

if True:
    W = 4
";
        assert_eq!(
            outline("a.py", source),
            [
                "variable X",
                "class C",
                "  variable y",
                "  method m",
                "function f"
            ]
        );
    }

    #[test]
    fn javascript_lists_declarations_fields_and_labeled_calls() {
        let source = "\
import x from 'y';
export const A = 1;
const f = () => {};
let b = 2, c = 3;
export default function g() {}
class K {
  field = 1;
  handler = () => {};
  m() {}
}
describe(\"suite\", () => {
  it(\"works\", () => {
    const inner = 1;
  });
});
if (x) { const no = 1; }
";
        assert_eq!(
            outline("a.js", source),
            [
                "variable A",
                "function f",
                "variable b",
                "variable c",
                "function g",
                "class K",
                "  variable field",
                "  method handler",
                "  method m",
                "test describe(\"suite\")",
                "  test it(\"works\")",
            ]
        );
    }

    #[test]
    fn typescript_lists_enums_namespaces_overloads_and_interface_methods() {
        let source = "\
export enum E { A }
namespace N {
  export function inside() {}
}
function o(a: string): void;
function o(a: any) {}
interface I {
  m(): void;
  p: string;
}
type T = { a: string };
abstract class A {
  abstract m(): void;
}
";
        assert_eq!(
            outline("a.ts", source),
            [
                "enum E",
                "namespace N",
                "  function inside",
                "function o",
                "function o",
                "interface I",
                "  method m",
                "type alias T",
                "class A",
                "  method m",
            ]
        );
    }

    #[test]
    fn go_lists_specs_receivers_and_subtests() {
        let source = "\
package p

import \"fmt\"

const A = 1

const (
\tB = 2
\tC = 3
)

var D = 4

type Server struct {
\ta int
}

func (s *Server) Start() {
\tconst E = 5
}

func Top() {}

func TestX(t *testing.T) {
\tt.Run(\"case\", func(t *testing.T) {})
}
";
        assert_eq!(
            outline("a.go", source),
            [
                "constant A",
                "constant B",
                "constant C",
                "variable D",
                "type Server",
                "method Server.Start",
                "function Top",
                "function TestX",
                "  test t.Run(\"case\")",
            ]
        );
    }

    #[test]
    fn ruby_lists_constants_methods_and_specs() {
        let source = "\
X = 1

module M
  class C
    Y = 2

    def run
      Z = 3
    end

    def self.build
    end
  end
end

RSpec.describe C do
  it \"works\" do
  end
end
";
        assert_eq!(
            outline("a.rb", source),
            [
                "constant X",
                "module M",
                "  class C",
                "    constant Y",
                "    method run",
                "    method self.build",
                "test RSpec.describe",
                "  test it(\"works\")",
            ]
        );
    }

    #[test]
    fn kotlin_lists_properties_objects_and_type_aliases() {
        let source = "\
val top = 1

class C {
    val x = 2

    fun m(a: Int): Int {
        return a
    }

    companion object {
        fun make() = C()
    }
}

object O {
    fun n() {}
}

typealias N = Int

fun f() {}
";
        assert_eq!(
            outline("a.kt", source),
            [
                "variable top",
                "class C",
                "  variable x",
                "  method m",
                "  method make",
                "object O",
                "  method n",
                "type alias N",
                "function f",
            ]
        );
    }

    #[test]
    fn java_lists_members_of_classes_and_interfaces() {
        let source = "\
class C {
  int x = 1;
  enum E { A }
  record R(int a) {}
  C() {}
  void m() {}
}
interface I {
  void n();
}
";
        assert_eq!(
            outline("A.java", source),
            [
                "class C",
                "  variable x",
                "  enum E",
                "  class R",
                "  constructor C",
                "  method m",
                "interface I",
                "  method n",
            ]
        );
    }

    #[test]
    fn c_lists_macros_prototypes_and_types_inside_include_guards() {
        let source = "\
#include <stdio.h>
#ifndef H
#define H
#define N 1
#define SQ(x) ((x)*(x))
int proto(int a);
int global;
typedef struct { int x; } P;
enum E { A };
struct point { int x; };
int f(void) { return 0; }
#endif
";
        assert_eq!(
            outline("a.c", source),
            [
                "macro H",
                "macro N",
                "macro SQ",
                "function proto",
                "type alias P",
                "enum E",
                "struct point",
                "function f",
            ]
        );
    }

    #[test]
    fn cpp_lists_namespaces_classes_and_member_declarations() {
        let source = "\
#include <vector>
namespace ns {
class Foo {
 public:
  int bar(int a);
  int x;
  using T = int;
  int inline_m() { return 1; }
};
int free_fn();
}
int ns::Foo::bar(int a) { return a; }
";
        assert_eq!(
            outline("a.cpp", source),
            [
                "namespace ns",
                "  class Foo",
                "    method bar",
                "    type alias T",
                "    method inline_m",
                "  function free_fn",
                "function ns::Foo::bar",
            ]
        );
    }

    #[test]
    fn bash_lists_functions() {
        let source = "source ./x.sh\nfoo() {\n  echo 1\n}\nfunction bar {\n  echo 2\n}\n";
        assert_eq!(outline("a.sh", source), ["function foo", "function bar"]);
    }

    #[test]
    fn lua_lists_functions_assigned_and_declared() {
        let source = "\
local M = {}
function M.foo() end
M.bar = function() return 1 end
local baz = function() end
local x = 1
describe(\"suite\", function()
  it(\"works\", function() end)
end)
return M
";
        assert_eq!(
            outline("a.lua", source),
            [
                "function M.foo",
                "function M.bar",
                "function baz",
                "test describe(\"suite\")",
                "  test it(\"works\")",
            ]
        );
    }

    #[test]
    fn css_lists_rules_and_at_rules() {
        let source = "\
@import url(x.css);
.a { color: red; }
@media (max-width: 600px) {
  .b { color: blue; }
}
@keyframes spin { from { x: 1 } }
";
        assert_eq!(
            outline("a.css", source),
            [
                "rule .a",
                "block @media (max-width: 600px)",
                "  rule .b",
                "block @keyframes spin",
            ]
        );
    }

    #[test]
    fn hcl_lists_blocks_and_attributes() {
        let source = "\
region = \"x\"
resource \"aws_s3_bucket\" \"logs\" {
  bucket = \"x\"
  lifecycle {
    prevent_destroy = true
  }
}
";
        assert_eq!(
            outline("a.tf", source),
            [
                "variable region",
                "block resource \"aws_s3_bucket\" \"logs\"",
                "  variable bucket",
                "  block lifecycle",
                "    variable prevent_destroy",
            ]
        );
    }

    #[test]
    fn markdown_lists_sections_nested_by_level() {
        let source = "Intro.\n\n# Title\n\nText.\n\n## Sub\n\nMore.\n\n# Next\n";
        assert_eq!(
            outline("a.md", source),
            ["section Title", "  section Sub", "section Next"]
        );
    }

    #[test]
    fn sql_lists_ddl_statements() {
        let source = "\
CREATE TABLE users (
  id int
);
ALTER TABLE users ADD COLUMN x int;
SELECT 1;
";
        assert_eq!(
            outline("a.sql", source),
            ["statement users", "statement users"]
        );
    }

    #[test]
    fn toml_lists_tables_and_their_keys() {
        let source = "\
top = 1
[package]
name = \"x\"
[[bin]]
name = \"a\"
[dependencies]
serde = { version = \"1\" }
";
        assert_eq!(
            outline("Cargo.toml", source),
            [
                "key top",
                "table package",
                "  key name",
                "table bin",
                "  key name",
                "table dependencies",
                "  key serde",
            ]
        );
    }

    /// The diff-case inputs: every `2-original.*` and `3-updated.*`.
    fn case_inputs() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diff_cases/cases");
        let mut inputs: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .flat_map(|case| std::fs::read_dir(case.unwrap().path()).unwrap())
            .map(|file| file.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("2-") || name.starts_with("3-"))
            })
            .collect();
        inputs.sort();
        inputs
    }

    /// Names of the scope definitions holding `line`, outermost first.
    /// Breadcrumbs walk up from the line's first token, so they miss a
    /// scope that starts later on its line (`export function f`,
    /// `const f = () =>`); those are left out.
    fn scopes_at(parsed: &ParsedFile, outline: &Outline, line: usize, first: usize) -> Vec<String> {
        outline
            .definitions
            .iter()
            .filter(|definition| {
                let starts_here = row_of(&parsed.lines, definition.node_start) == line;
                definition.scope
                    && definition.rows.contains(&line)
                    && (!starts_here || definition.node_start <= first)
            })
            .map(|definition| definition.name.clone())
            .collect()
    }

    /// Breadcrumb names at `line` that name definitions.
    fn crumbs_at(parsed: &ParsedFile, line: usize) -> Vec<String> {
        let kinds = parsed.config.definition_kinds;
        parsed
            .breadcrumb_scopes(line)
            .into_iter()
            .filter(|scope| {
                !scope.name.is_empty() && kinds.iter().any(|(kind, _)| *kind == scope.kind)
            })
            .map(|scope| scope.name)
            .collect()
    }

    /// Lines of `source` where the scope definitions and the breadcrumbs
    /// disagree.
    fn disagreements(name: &str, source: &str) -> Vec<String> {
        let Some(parsed) = ParsedFile::parse(name, source) else {
            return Vec::new();
        };
        let outline = parsed.outline();
        let mut out = Vec::new();
        for (line, text) in source.lines().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let first = parsed.lines[line].start + (text.len() - text.trim_start().len());
            let defined = scopes_at(&parsed, &outline, line, first);
            let breadcrumb = crumbs_at(&parsed, line);
            if defined != breadcrumb {
                out.push(format!(
                    "{name}:{}: {text:?}\n  outline {defined:?}\n  crumbs  {breadcrumb:?}",
                    line + 1
                ));
            }
        }
        out
    }

    #[test]
    fn scope_definitions_agree_with_breadcrumbs_on_every_line() {
        let failures: Vec<String> = case_inputs()
            .iter()
            .flat_map(|path| {
                let source = std::fs::read_to_string(path).unwrap();
                disagreements(path.to_str().unwrap(), &source)
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn json_and_yaml_list_keys_two_levels_deep() {
        let json = "{\"a\": {\"b\": {\"c\": 1}}, \"d\": [{\"e\": 1}]}\n";
        assert_eq!(outline("a.json", json), ["key a", "  key b", "key d"]);
        let yaml = "a:\n  b:\n    c: 1\nd:\n  - e: 1\n";
        assert_eq!(outline("a.yaml", yaml), ["key a", "  key b", "key d"]);
    }
}
