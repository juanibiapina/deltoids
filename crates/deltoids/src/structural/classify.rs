//! Classify pairings into [`StructuralChange`]s and render human-readable
//! descriptions.
//!
//! Stage three of the structural diff. Takes a `Vec<Pairing>` (from
//! [`super::pair_symbols`]) and produces a `Vec<StructuralChange>` where
//! every entry has a precise [`ChangeKind`] (Added / Removed / Modified
//! / Renamed / SignatureChanged / VisibilityChanged / BodyChanged) and a
//! description suitable for direct display ("Added method `Foo::bar`").
//!
//! Classification rules for `Pairing::Match`:
//! * If the symbols are byte-equal in signature, body span, and
//!   visibility, no change is emitted (the structural diff hides
//!   identical symbols).
//! * If only the body interior changed (signature, visibility, kind,
//!   span boundaries the same), emit `BodyChanged`.
//! * If only the signature changed, emit `SignatureChanged`.
//! * If only the visibility changed, emit `VisibilityChanged`.
//! * Otherwise emit `Modified` (covers signature + body simultaneously,
//!   span growth, etc.).
//!
//! Classification for the one-sided cases is mechanical:
//! * `Pairing::OldOnly` → `Removed`.
//! * `Pairing::NewOnly` → `Added`.
//! * `Pairing::Rename` → `Renamed`.

use super::pair::Pairing;
use super::symbol::{Symbol, SymbolKind, Visibility};

/// What kind of structural change a pair represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    /// Catch-all for non-trivial modifications that affect both
    /// signature and body, or that don't fit the more specific
    /// variants. Always populates both `before` and `after`.
    Modified,
    /// Same kind, same path, only the body interior changed.
    BodyChanged,
    /// Same kind, same path, only the signature changed (parameters,
    /// return type, name attributes).
    SignatureChanged,
    /// Same kind, same path, only the visibility changed.
    VisibilityChanged,
    /// Path changed (rename detected via signature similarity).
    Renamed,
}

/// One structural change with its associated symbols and description.
#[derive(Debug, Clone)]
pub struct StructuralChange {
    pub kind: ChangeKind,
    /// The pre-change symbol; `None` only for `Added`.
    pub before: Option<Symbol>,
    /// The post-change symbol; `None` only for `Removed`.
    pub after: Option<Symbol>,
    pub description: String,
}

impl StructuralChange {
    /// True when this change is visible in a public-only view: the
    /// symbol must be public on at least one side. This matches typical
    /// API-review intent: any change touching a public surface is shown.
    pub fn is_public(&self) -> bool {
        let bef_public = self
            .before
            .as_ref()
            .map(|s| s.visibility == Visibility::Public)
            .unwrap_or(false);
        let aft_public = self
            .after
            .as_ref()
            .map(|s| s.visibility == Visibility::Public)
            .unwrap_or(false);
        bef_public || aft_public
    }

    /// The "primary" symbol of this change — the post-change one if
    /// it exists, otherwise the pre-change one. Useful for jumping
    /// from a description to the file location.
    pub fn primary(&self) -> &Symbol {
        self.after
            .as_ref()
            .or(self.before.as_ref())
            .expect("a change always has at least one symbol")
    }
}

/// Convert pairings into structural changes. Identical pairs (no diff)
/// are dropped.
pub fn classify(pairings: Vec<Pairing>) -> Vec<StructuralChange> {
    let mut out = Vec::new();
    for pairing in pairings {
        match pairing {
            Pairing::Match { old, new } => {
                if let Some(change) = classify_match(old, new) {
                    out.push(change);
                }
            }
            Pairing::Rename { old, new, .. } => out.push(make_renamed(old, new)),
            Pairing::OldOnly(s) => out.push(make_removed(s)),
            Pairing::NewOnly(s) => out.push(make_added(s)),
        }
    }
    out
}

fn classify_match(old: Symbol, new: Symbol) -> Option<StructuralChange> {
    let signature_changed = old.core_signature() != new.core_signature();
    let visibility_changed = old.visibility != new.visibility;
    let body_changed = body_text_differs(&old, &new);

    if !signature_changed && !visibility_changed && !body_changed {
        return None;
    }

    let kind = match (signature_changed, visibility_changed, body_changed) {
        (true, false, false) => ChangeKind::SignatureChanged,
        (false, true, false) => ChangeKind::VisibilityChanged,
        (false, false, true) => ChangeKind::BodyChanged,
        _ => ChangeKind::Modified,
    };

    let description = describe_match(&old, &new, kind);
    Some(StructuralChange {
        kind,
        before: Some(old),
        after: Some(new),
        description,
    })
}

/// True when the bodies differ in **content**. Span-only changes
/// (e.g. a function moved lines because something was inserted above)
/// are not body changes — we ignore them so a single edit doesn't
/// flag every symbol below as Modified.
fn body_text_differs(old: &Symbol, new: &Symbol) -> bool {
    old.body_text != new.body_text
}

fn make_added(s: Symbol) -> StructuralChange {
    let description = format!(
        "Added {} `{}`{}",
        kind_word(&s.kind),
        s.qualified_name(),
        visibility_suffix(&s.visibility)
    );
    StructuralChange {
        kind: ChangeKind::Added,
        before: None,
        after: Some(s),
        description,
    }
}

fn make_removed(s: Symbol) -> StructuralChange {
    let description = format!(
        "Removed {} `{}`{}",
        kind_word(&s.kind),
        s.qualified_name(),
        visibility_suffix(&s.visibility)
    );
    StructuralChange {
        kind: ChangeKind::Removed,
        before: Some(s),
        after: None,
        description,
    }
}

fn make_renamed(old: Symbol, new: Symbol) -> StructuralChange {
    let description = format!(
        "Renamed {} `{}` → `{}`",
        kind_word(&new.kind),
        old.qualified_name(),
        new.qualified_name()
    );
    StructuralChange {
        kind: ChangeKind::Renamed,
        before: Some(old),
        after: Some(new),
        description,
    }
}

fn describe_match(old: &Symbol, new: &Symbol, kind: ChangeKind) -> String {
    let label = match kind {
        ChangeKind::SignatureChanged => "Changed signature of",
        ChangeKind::VisibilityChanged => "Changed visibility of",
        ChangeKind::BodyChanged => "Modified",
        ChangeKind::Modified => "Modified",
        _ => "Modified",
    };
    let mut s = format!(
        "{} {} `{}`",
        label,
        kind_word(&new.kind),
        new.qualified_name()
    );
    if matches!(kind, ChangeKind::VisibilityChanged) {
        s.push_str(&format!(
            " ({} → {})",
            visibility_word(&old.visibility),
            visibility_word(&new.visibility)
        ));
    }
    s
}

/// Lower-case noun for a symbol kind, used in descriptions.
pub fn kind_word(k: &SymbolKind) -> &'static str {
    match k {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Type => "type",
        SymbolKind::Const => "constant",
        SymbolKind::Module => "module",
        SymbolKind::Field => "field",
        SymbolKind::Macro => "macro",
        SymbolKind::Impl => "impl",
        SymbolKind::Other(_) => "item",
    }
}

fn visibility_word(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Crate => "crate",
        Visibility::Restricted(_) => "restricted",
    }
}

fn visibility_suffix(v: &Visibility) -> String {
    match v {
        Visibility::Public => " (public)".into(),
        Visibility::Private => String::new(),
        Visibility::Crate => " (crate)".into(),
        Visibility::Restricted(s) => format!(" ({s})"),
    }
}

#[cfg(test)]
mod tests;
