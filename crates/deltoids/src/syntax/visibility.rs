//! Who can use a definition. Each definition's own syntax says what it
//! declares ([`ParsedFile::declared_visibility`], by the language's
//! [`VisibilityRule`]); [`resolve`] caps it by the definition that holds
//! it, so a `pub fn` in a private module is private.

use tree_sitter::Node;

use super::outline::Definition;
use super::{Field, ParsedFile};
use crate::definitions::{DefinitionKind, Visibility};
use crate::language::VisibilityRule;

/// What a definition's own syntax says about who can use it.
#[derive(Debug, Clone)]
pub(super) enum Declared {
    /// Its language or its kind has no visibility.
    Unknown,
    /// This, capped by its container's.
    Own(Visibility),
    /// Its container's.
    Inherit,
    /// A Rust `impl`: the visibility of the type it names, when the file
    /// defines that type.
    OfType(String),
    /// A JavaScript or TypeScript top-level definition without `export`:
    /// private in a module, none in a script.
    Unexported,
}

impl ParsedFile {
    /// What `node`, a definition of `kind` named `name` inside a definition
    /// of `container` kind, declares. `ancestors` are the nodes between it
    /// and its container, innermost first, the container's node included.
    pub(super) fn declared_visibility(
        &self,
        node: Node,
        kind: DefinitionKind,
        name: &str,
        container: Option<DefinitionKind>,
        ancestors: &[Node],
    ) -> Declared {
        if kind == DefinitionKind::Test {
            return Declared::Unknown;
        }
        match self.config.visibility {
            VisibilityRule::None => Declared::Unknown,
            VisibilityRule::Rust => self.rust_visibility(node, ancestors),
            VisibilityRule::Exported => self.exported_visibility(node, container, ancestors),
            VisibilityRule::Capitalized => {
                let exported = last_segment(name).starts_with(|c: char| c.is_uppercase());
                Declared::Own(if exported {
                    Visibility::Public
                } else {
                    Visibility::Private
                })
            }
            VisibilityRule::Underscore => {
                let name = last_segment(name);
                let dunder = name.len() > 4 && name.starts_with("__") && name.ends_with("__");
                Declared::Own(if name.starts_with('_') && !dunder {
                    Visibility::Private
                } else {
                    Visibility::Public
                })
            }
        }
    }

    fn rust_visibility(&self, node: Node, ancestors: &[Node]) -> Declared {
        match node.kind() {
            "impl_item" => {
                return self
                    .child(node, Field::Type)
                    .map(|ty| Declared::OfType(type_name(self.text(ty))))
                    .unwrap_or(Declared::Unknown);
            }
            "macro_definition" => {
                return Declared::Own(if self.macro_exported(node) {
                    Visibility::Public
                } else {
                    Visibility::Private
                });
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let modifier = node
            .children(&mut cursor)
            .find(|child| child.kind() == "visibility_modifier");
        if let Some(modifier) = modifier {
            return Declared::Own(if self.text(modifier) == "pub" {
                Visibility::Public
            } else {
                Visibility::Restricted
            });
        }
        let holder = ancestors
            .iter()
            .find(|ancestor| matches!(ancestor.kind(), "impl_item" | "trait_item"));
        match holder {
            Some(holder) if holder.kind() == "trait_item" => Declared::Inherit,
            Some(holder) if self.child(*holder, Field::Trait).is_some() => {
                Declared::Own(Visibility::Public)
            }
            _ => Declared::Own(Visibility::Private),
        }
    }

    /// `#[macro_export]` among the attributes above `node`.
    fn macro_exported(&self, node: Node) -> bool {
        let mut sibling = node.prev_named_sibling();
        while let Some(above) = sibling {
            match above.kind() {
                "attribute_item" if self.text(above).contains("macro_export") => return true,
                "attribute_item" | "line_comment" | "block_comment" => {}
                _ => return false,
            }
            sibling = above.prev_named_sibling();
        }
        false
    }

    fn exported_visibility(
        &self,
        node: Node,
        container: Option<DefinitionKind>,
        ancestors: &[Node],
    ) -> Declared {
        match container {
            None | Some(DefinitionKind::Namespace | DefinitionKind::Module) => {
                let exported = ancestors
                    .iter()
                    .any(|ancestor| ancestor.kind() == "export_statement");
                match (exported, container) {
                    (true, _) => Declared::Own(Visibility::Public),
                    (false, None) => Declared::Unexported,
                    (false, Some(_)) => Declared::Own(Visibility::Private),
                }
            }
            Some(_) => {
                let mut cursor = node.walk();
                let modifier = node
                    .children(&mut cursor)
                    .find(|child| child.kind() == "accessibility_modifier");
                match modifier.map(|modifier| self.text(modifier)) {
                    Some("private") => return Declared::Own(Visibility::Private),
                    Some("protected") => return Declared::Own(Visibility::Restricted),
                    _ => {}
                }
                let name = self
                    .name_node(node)
                    .or_else(|| self.child(node, Field::Property));
                if name.is_some_and(|name| name.kind() == "private_property_identifier") {
                    Declared::Own(Visibility::Private)
                } else {
                    Declared::Inherit
                }
            }
        }
    }

    /// Whether the file is a JavaScript or TypeScript module: it exports
    /// something.
    pub(super) fn exports(&self) -> bool {
        let root = self.tree.root_node();
        let mut cursor = root.walk();
        root.children(&mut cursor)
            .any(|child| child.kind() == "export_statement")
    }

    fn text(&self, node: Node) -> &str {
        self.source.get(node.byte_range()).unwrap_or("")
    }
}

/// Each definition's visibility from what it `declared`, in document order
/// (a container before the definitions it holds). `module` says whether a
/// JavaScript or TypeScript file exports anything.
pub(super) fn resolve(definitions: &mut [Definition], declared: &[Declared], module: bool) {
    for index in 0..definitions.len() {
        let container = definitions[index]
            .parent
            .and_then(|parent| definitions[parent].visibility);
        definitions[index].visibility = match &declared[index] {
            Declared::Unknown => None,
            Declared::Own(own) => Some(container.map_or(*own, |container| (*own).min(container))),
            Declared::Inherit => container,
            Declared::Unexported => module.then_some(Visibility::Private),
            Declared::OfType(name) => {
                definitions
                    .iter()
                    .zip(declared)
                    .find_map(|(definition, declared)| match declared {
                        Declared::Own(own)
                            if definition.parent.is_none() && definition.name == *name =>
                        {
                            Some(*own)
                        }
                        _ => None,
                    })
            }
        };
    }
}

/// The part of a name after its last `.`: `Start` in `Server.Start`.
fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// The type a Rust `impl` names, without its path or type arguments:
/// `Cart` for `crate::shop::Cart<T>`.
fn type_name(text: &str) -> String {
    let base = text.split('<').next().unwrap_or(text);
    base.rsplit("::").next().unwrap_or(base).trim().to_string()
}
