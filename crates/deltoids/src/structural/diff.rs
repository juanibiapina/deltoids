//! Top-level [`StructuralDiff`] API.
//!
//! Stage four of the structural-diff layer. Ties together the phases
//! built earlier:
//!
//! 1. [`crate::structural::symbol::extract_symbols`] — walk the AST.
//! 2. [`crate::structural::pair::pair_symbols`] — pair old/new.
//! 3. [`crate::structural::classify::classify`] — produce changes.
//!
//! `StructuralDiff::compute(original, updated, path)` runs the whole
//! pipeline and returns a value that views can render directly. The
//! object also remembers the detected language (so consumers can avoid
//! re-detecting when annotating hunks) and supports filter accessors:
//!
//! - `changes()` — all structural changes, in source order on the new
//!   side (added at their location, removed at their old location).
//! - `public_changes()` — only changes that touch a public symbol.
//! - `signature_changes()` — only Added / Removed / Renamed /
//!   SignatureChanged / VisibilityChanged (drops body-only changes).

use super::classify::{ChangeKind, StructuralChange, classify};
use super::pair::pair_symbols;
use super::symbol::extract_symbols;
use crate::Language;

/// A structural diff between two snapshots of the same file.
#[derive(Debug, Clone, Default)]
pub struct StructuralDiff {
    changes: Vec<StructuralChange>,
    language: Option<Language>,
}

impl StructuralDiff {
    /// Compute the structural diff between `original` and `updated`.
    /// Returns an empty diff (no changes, no language) if the language
    /// is not supported or the source can't be parsed on either side.
    pub fn compute(original: &str, updated: &str, path: &str) -> Self {
        let language = Language::detect(path, updated).or_else(|| Language::detect(path, original));
        let old = extract_symbols(path, original);
        let new = extract_symbols(path, updated);
        let mut changes = classify(pair_symbols(old, new));
        changes.sort_by_key(sort_key);
        Self { changes, language }
    }

    /// Detected language used to extract symbols. `None` for files we
    /// can't parse (and hence can't structurally diff).
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// All structural changes in source order.
    pub fn changes(&self) -> &[StructuralChange] {
        &self.changes
    }

    /// Iterator over only the changes that touch a public symbol.
    pub fn public_changes(&self) -> impl Iterator<Item = &StructuralChange> {
        self.changes.iter().filter(|c| c.is_public())
    }

    /// Iterator over only signature-affecting changes (drops body-only
    /// modifications). Use this for an "API change" view.
    pub fn signature_changes(&self) -> impl Iterator<Item = &StructuralChange> {
        self.changes.iter().filter(|c| !is_body_only(c))
    }

    /// Number of changes, total.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// True when there are no structural changes.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Body-only when the only thing the change reports is a body-interior
/// movement; we collapse `Modified` here too because, by construction,
/// `Modified` includes the body.
fn is_body_only(c: &StructuralChange) -> bool {
    matches!(c.kind, ChangeKind::BodyChanged)
}

/// Sort changes for stable display: by the new-side line where the
/// symbol lives, falling back to the old-side line if the new is
/// missing (Removed). This puts the changes roughly in scroll order,
/// which is what reviewers expect.
fn sort_key(c: &StructuralChange) -> (usize, usize, usize) {
    let new_line = c.after.as_ref().map(|s| s.span.start).unwrap_or(usize::MAX);
    let old_line = c
        .before
        .as_ref()
        .map(|s| s.span.start)
        .unwrap_or(usize::MAX);
    let priority = match c.kind {
        ChangeKind::Added => 0,
        ChangeKind::Modified | ChangeKind::SignatureChanged | ChangeKind::BodyChanged => 1,
        ChangeKind::VisibilityChanged => 1,
        ChangeKind::Renamed => 1,
        ChangeKind::Removed => 2,
    };
    (new_line.min(old_line), priority, old_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_empty_when_no_changes() {
        let d = StructuralDiff::compute("fn x() {}\n", "fn x() {}\n", "a.rs");
        assert!(d.is_empty());
        assert_eq!(d.language(), Some(crate::Language::Rust));
    }

    #[test]
    fn compute_picks_up_added_function() {
        let d = StructuralDiff::compute("fn x() {}\n", "fn x() {}\nfn y() {}\n", "a.rs");
        assert_eq!(d.changes().len(), 1);
        assert_eq!(d.changes()[0].kind, ChangeKind::Added);
    }

    #[test]
    fn public_changes_filter_drops_private() {
        let d = StructuralDiff::compute(
            "fn priv_a() {}\npub fn pub_a() {}\n",
            "fn priv_a() { let x = 1; }\npub fn pub_a() { let y = 2; }\n",
            "a.rs",
        );
        assert_eq!(d.public_changes().count(), 1);
    }

    #[test]
    fn signature_changes_filter_drops_body_only() {
        let d = StructuralDiff::compute(
            "pub fn x(a: i32) -> i32 { a }\n",
            "pub fn x(a: i32) -> i32 { a + 1 }\n",
            "a.rs",
        );
        assert_eq!(d.changes().len(), 1, "expect a body-changed match");
        assert_eq!(d.changes()[0].kind, ChangeKind::BodyChanged);
        assert_eq!(d.signature_changes().count(), 0);
    }

    #[test]
    fn unsupported_language_yields_empty_diff() {
        let d = StructuralDiff::compute("anything", "different", "data.unknown");
        assert!(d.is_empty());
        assert_eq!(d.language(), None);
    }

    #[test]
    fn changes_sorted_by_source_line() {
        let old = "\
fn alpha() {}
fn beta() {}
fn gamma() {}
";
        let new = "\
fn alpha() {}
fn newer() {}
fn beta() {}
fn gamma() {}
";
        let d = StructuralDiff::compute(old, new, "a.rs");
        let descs: Vec<_> = d.changes().iter().map(|c| c.description.clone()).collect();
        assert_eq!(descs, vec!["Added function `newer`"]);
    }
}
