//! File-structure outline with per-symbol diff status.
//!
//! Where [`super::diff::StructuralDiff`] surfaces only **changed**
//! symbols, [`outline`] returns the full structural skeleton of the
//! file — every class, struct, trait, function, method, type, etc. —
//! with each entry tagged by its diff status (`Unchanged` / `Added` /
//! `Removed` / `Modified` / `SignatureChanged` / `VisibilityChanged` /
//! `BodyChanged` / `Renamed`).
//!
//! Consumers (rv's "Outline" view; future surfaces) render this as a
//! tree-style list with diff-coloured backgrounds, so reviewers see
//! both the file's structure *and* what moved within it. This is the
//! difference between "here are the 3 changes" (the summary view) and
//! "here is the whole file, and here is which 3 of its 27 symbols
//! moved" (this view).
//!
//! Layout:
//! - The spine is the **new-side** symbols in source order (so the
//!   outline reads top-to-bottom matching the file the user is looking
//!   at).
//! - Removed symbols are inserted just after their old left-sibling,
//!   so they appear roughly where they used to live.
//! - Each entry carries its `depth` (path length minus one) so the
//!   renderer can indent without reparsing the path.
//!
//! The pairing reuses [`super::pair::pair_symbols`] so renames and
//! signature similarity flow through to status assignment.

use super::pair::{Pairing, pair_symbols};
use super::symbol::{Symbol, SymbolKind, SymbolPath, Visibility, extract_symbols};

/// Per-symbol diff status. Ranks roughly by visual prominence:
/// `Unchanged` is the calm baseline, `Added`/`Removed` are loud,
/// `Modified` flavours sit between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineStatus {
    Unchanged,
    Added,
    Removed,
    /// Same name + kind, body interior changed. Most common Modified
    /// flavour; reviewers can scroll to the hunks for detail.
    BodyChanged,
    /// Same name + kind, signature changed (parameters, return type).
    SignatureChanged,
    /// Same name + kind, visibility changed (private → public, etc.).
    VisibilityChanged,
    /// Same path, multiple kinds of change at once. Catch-all.
    Modified,
    /// Path changed (rename detected via signature similarity).
    Renamed,
}

impl OutlineStatus {
    /// True for any status other than `Unchanged`. Useful for
    /// filtering to "things that moved".
    pub fn is_change(self) -> bool {
        !matches!(self, OutlineStatus::Unchanged)
    }
}

/// One row in the outline. The renderer turns each into a single
/// styled line.
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub kind: SymbolKind,
    /// Qualified path (`["Foo", "bar"]`).
    pub path: SymbolPath,
    /// Raw declaration signature for the entry's "current" side
    /// (post-change for everything but `Removed`, where the pre-change
    /// signature is shown).
    pub signature: String,
    pub visibility: Visibility,
    pub status: OutlineStatus,
    /// 0 = top-level, 1 = nested one level (method of class, etc.).
    /// Computed from path length.
    pub depth: usize,
    /// Line on the new side, when the symbol exists post-change.
    pub new_line: Option<usize>,
    /// Full new-side span (declaration + body + closing brace) when
    /// the symbol exists post-change. Used by the outline renderer to
    /// elide function bodies.
    pub new_span: Option<crate::structural::symbol::LineSpan>,
    /// New-side body interior span when present. Excludes opening /
    /// closing delimiters.
    pub new_body_span: Option<crate::structural::symbol::LineSpan>,
    /// Line on the old side, when the symbol existed pre-change.
    pub old_line: Option<usize>,
    /// For `Renamed` rows, the symbol's old qualified name.
    pub renamed_from: Option<SymbolPath>,
}

impl OutlineEntry {
    /// Convenience for renderers: the qualified name to display.
    pub fn qualified_name(&self) -> String {
        self.path.join("::")
    }
}

/// Compute the file outline. Returns an empty vec for unsupported
/// languages (no symbols extracted on either side).
pub fn outline(original: &str, updated: &str, path: &str) -> Vec<OutlineEntry> {
    let old = extract_symbols(path, original);
    let new = extract_symbols(path, updated);

    if old.is_empty() && new.is_empty() {
        return Vec::new();
    }

    // Remember every old symbol's source position so we can splice
    // removed entries back in after the right neighbour.
    let old_positions: Vec<(SymbolPath, usize)> =
        old.iter().map(|s| (s.path.clone(), s.span.start)).collect();

    let pairings = pair_symbols(old, new);

    // Step 1: build new-side entries (Match / Rename / NewOnly).
    let mut new_side_entries: Vec<OutlineEntry> = Vec::new();
    let mut removed: Vec<(Symbol, usize)> = Vec::new(); // (symbol, original old-index)

    for pairing in pairings {
        match pairing {
            Pairing::Match { old, new } => {
                let status = match_status(&old, &new);
                new_side_entries.push(entry_from_match(old, new, status));
            }
            Pairing::Rename {
                old,
                new,
                similarity: _,
            } => {
                new_side_entries.push(entry_from_rename(old, new));
            }
            Pairing::NewOnly(s) => {
                new_side_entries.push(entry_from_added(s));
            }
            Pairing::OldOnly(s) => {
                let old_idx = old_positions
                    .iter()
                    .position(|(p, _)| p == &s.path)
                    .unwrap_or(usize::MAX);
                removed.push((s, old_idx));
            }
        }
    }

    // Step 2: sort new-side entries by their new_line so the spine
    // reads in source order.
    new_side_entries.sort_by_key(|e| e.new_line.unwrap_or(usize::MAX));

    // Step 3: splice removed entries back in. For each removed symbol,
    // find the old-side neighbour with the smallest old-position that's
    // still ≤ this removed symbol's old position AND survived as a
    // new-side entry. Insert the removed entry just after that
    // neighbour. If no such neighbour exists, prepend.
    splice_removed(new_side_entries, removed)
}

/// Insert `removed` symbols into `entries` after their left old-neighbour.
fn splice_removed(
    entries: Vec<OutlineEntry>,
    mut removed: Vec<(Symbol, usize)>,
) -> Vec<OutlineEntry> {
    if removed.is_empty() {
        return entries;
    }
    // Sort removed by old position so their relative ordering is
    // preserved if multiple land at the same insertion point.
    removed.sort_by_key(|(_, idx)| *idx);

    // Index existing entries by old-side path → position in `entries`,
    // so we can locate left-neighbours quickly.
    let mut path_to_pos: std::collections::HashMap<SymbolPath, usize> =
        std::collections::HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let key = match e.status {
            OutlineStatus::Renamed => match &e.renamed_from {
                Some(old) => old.clone(),
                None => e.path.clone(),
            },
            _ => e.path.clone(),
        };
        path_to_pos.insert(key, i);
    }

    let mut out = entries;
    for (sym, _) in removed {
        let entry = entry_from_removed(sym);
        let left = find_left_old_neighbour(&entry.path, &path_to_pos);
        let insert_at = left.map(|pos| pos + 1).unwrap_or(0);
        insert_with_shift(&mut out, &mut path_to_pos, insert_at, entry);
    }
    out
}

/// Insert `entry` at `insert_at`, shifting every existing index
/// `≥ insert_at` in `path_to_pos` by one and registering the new
/// entry's path. Centralised so the splice loop stays flat.
fn insert_with_shift(
    out: &mut Vec<OutlineEntry>,
    path_to_pos: &mut std::collections::HashMap<SymbolPath, usize>,
    insert_at: usize,
    entry: OutlineEntry,
) {
    out.insert(insert_at, entry);
    for v in path_to_pos.values_mut() {
        if *v >= insert_at {
            *v += 1;
        }
    }
    path_to_pos.insert(out[insert_at].path.clone(), insert_at);
}

/// Find the entry that should sit just before a removed symbol. We
/// look for an entry whose path shares the same parent and is
/// considered "earlier" in some stable sense. The simplest rule that
/// keeps removed siblings of unchanged classes near their friends is:
/// pick the same-parent entry with the largest existing index. Falls
/// back to `None` when no same-parent entry exists.
fn find_left_old_neighbour(
    removed_path: &SymbolPath,
    path_to_pos: &std::collections::HashMap<SymbolPath, usize>,
) -> Option<usize> {
    let parent = if removed_path.is_empty() {
        return None;
    } else {
        &removed_path[..removed_path.len() - 1]
    };
    let mut best: Option<usize> = None;
    for (path, pos) in path_to_pos {
        let path_parent = if path.is_empty() {
            &[][..]
        } else {
            &path[..path.len() - 1]
        };
        if path_parent != parent {
            continue;
        }
        best = Some(best.map_or(*pos, |cur| cur.max(*pos)));
    }
    best
}

fn match_status(old: &Symbol, new: &Symbol) -> OutlineStatus {
    let signature_changed = old.core_signature() != new.core_signature();
    let visibility_changed = old.visibility != new.visibility;
    let body_changed = old.body_text != new.body_text;
    match (signature_changed, visibility_changed, body_changed) {
        (false, false, false) => OutlineStatus::Unchanged,
        (true, false, false) => OutlineStatus::SignatureChanged,
        (false, true, false) => OutlineStatus::VisibilityChanged,
        (false, false, true) => OutlineStatus::BodyChanged,
        _ => OutlineStatus::Modified,
    }
}

fn entry_from_match(old: Symbol, new: Symbol, status: OutlineStatus) -> OutlineEntry {
    OutlineEntry {
        depth: new.path.len().saturating_sub(1),
        kind: new.kind.clone(),
        path: new.path.clone(),
        signature: new.signature,
        visibility: new.visibility,
        status,
        new_line: Some(new.span.start),
        new_span: Some(new.span),
        new_body_span: new.body_span,
        old_line: Some(old.span.start),
        renamed_from: None,
    }
}

fn entry_from_rename(old: Symbol, new: Symbol) -> OutlineEntry {
    OutlineEntry {
        depth: new.path.len().saturating_sub(1),
        kind: new.kind.clone(),
        path: new.path.clone(),
        signature: new.signature,
        visibility: new.visibility,
        status: OutlineStatus::Renamed,
        new_line: Some(new.span.start),
        new_span: Some(new.span),
        new_body_span: new.body_span,
        old_line: Some(old.span.start),
        renamed_from: Some(old.path),
    }
}

fn entry_from_added(s: Symbol) -> OutlineEntry {
    OutlineEntry {
        depth: s.path.len().saturating_sub(1),
        kind: s.kind.clone(),
        path: s.path.clone(),
        signature: s.signature,
        visibility: s.visibility,
        status: OutlineStatus::Added,
        new_line: Some(s.span.start),
        new_span: Some(s.span),
        new_body_span: s.body_span,
        old_line: None,
        renamed_from: None,
    }
}

fn entry_from_removed(s: Symbol) -> OutlineEntry {
    OutlineEntry {
        depth: s.path.len().saturating_sub(1),
        kind: s.kind.clone(),
        path: s.path.clone(),
        signature: s.signature,
        visibility: s.visibility,
        status: OutlineStatus::Removed,
        new_line: None,
        new_span: None,
        new_body_span: None,
        old_line: Some(s.span.start),
        renamed_from: None,
    }
}

#[cfg(test)]
mod tests;
