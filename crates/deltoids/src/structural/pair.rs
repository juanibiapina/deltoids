//! Pair old-side symbols with new-side symbols.
//!
//! Stage two of the structural diff. Takes two `Vec<Symbol>` (from
//! [`super::extract_symbols`]) and produces a list of [`SymbolPairing`]s:
//! every old symbol is matched against at most one new symbol, every new
//! symbol against at most one old. Pairings come in three flavours:
//! [`Pairing::Match`] (same qualified path, treated as the same symbol),
//! [`Pairing::Rename`] (cross-path match by signature similarity), or a
//! one-sided [`Pairing::OldOnly`] / [`Pairing::NewOnly`].
//!
//! The classifier in [`super::classify`] consumes pairings to decide
//! Added / Removed / Modified / Renamed / SignatureChanged etc. The
//! visibility filter ("only public") is applied later, on the
//! `StructuralChange` list, not here — pairings stay complete so
//! consumers can switch views without recomputing.
//!
//! The algorithm is intentionally simple:
//! 1. Hash every old symbol by its qualified path.
//! 2. For every new symbol, look up the same path; on hit, emit a
//!    `Match`. On miss, defer.
//! 3. After the path pass, run rename detection over the leftover
//!    old / new symbols using normalized signature similarity (≥ 0.6
//!    by default).
//!
//! This mirrors difftastic's "match liberally first, novel last" stance
//! without pulling in the full Dijkstra graph search: we don't have
//! difftastic's atom-level granularity goals, but we do want stable
//! same-path matches as the spine of every diff.

use super::symbol::Symbol;

/// A single pairing produced by [`pair_symbols`].
#[derive(Debug, Clone, PartialEq)]
pub enum Pairing {
    /// Same qualified path on both sides. Most common.
    Match { old: Symbol, new: Symbol },
    /// No path match, but signatures are similar enough to consider
    /// this a renamed declaration.
    Rename {
        old: Symbol,
        new: Symbol,
        /// Normalized similarity in `0.0 ..= 1.0`.
        similarity: f32,
    },
    /// Old-side symbol with no match on the new side. Treated as
    /// removed by the classifier.
    OldOnly(Symbol),
    /// New-side symbol with no match on the old side. Treated as added
    /// by the classifier.
    NewOnly(Symbol),
}

/// Default minimum signature similarity to consider a rename.
const RENAME_SIMILARITY: f32 = 0.6;

/// Pair old-side symbols against new-side symbols.
pub fn pair_symbols(old: Vec<Symbol>, new: Vec<Symbol>) -> Vec<Pairing> {
    pair_symbols_with(old, new, RENAME_SIMILARITY)
}

/// Like [`pair_symbols`] but with a tunable rename threshold. Mostly
/// useful for tests that want to stress one side or the other.
pub fn pair_symbols_with(
    old: Vec<Symbol>,
    new: Vec<Symbol>,
    rename_threshold: f32,
) -> Vec<Pairing> {
    let mut pairings: Vec<Pairing> = Vec::new();
    let mut old: Vec<Option<Symbol>> = old.into_iter().map(Some).collect();
    let mut new: Vec<Option<Symbol>> = new.into_iter().map(Some).collect();

    // Pass 1: same qualified path → Match.
    let new_len = new.len();
    #[allow(clippy::needless_range_loop)]
    for new_idx in 0..new_len {
        let new_path;
        let new_kind;
        match new[new_idx].as_ref() {
            Some(s) => {
                new_path = s.path.clone();
                new_kind = s.kind.clone();
            }
            None => continue,
        }
        let Some(old_idx) = old.iter().position(|o| {
            o.as_ref()
                .map(|s| s.path == new_path && s.kind == new_kind)
                .unwrap_or(false)
        }) else {
            continue;
        };
        let old_sym = old[old_idx].take().unwrap();
        let new_sym = new[new_idx].take().unwrap();
        pairings.push(Pairing::Match {
            old: old_sym,
            new: new_sym,
        });
    }

    // Pass 2: rename detection on leftovers.
    let mut leftover_old: Vec<usize> = (0..old.len()).filter(|&i| old[i].is_some()).collect();
    let leftover_new: Vec<usize> = (0..new.len()).filter(|&i| new[i].is_some()).collect();

    // Greedy best-match by similarity. Compute a similarity matrix only
    // for compatible pairs (same kind or same parent path) to keep the
    // search small and to avoid noise across categories.
    for &n_idx in &leftover_new {
        let Some(new_sym) = new[n_idx].as_ref() else {
            continue;
        };
        let mut best: Option<(usize, f32)> = None;
        for &o_idx in &leftover_old {
            let Some(old_sym) = old[o_idx].as_ref() else {
                continue;
            };
            if !pairing_compatible(old_sym, new_sym) {
                continue;
            }
            let sim = signature_similarity(&old_sym.signature, &new_sym.signature);
            if sim < rename_threshold {
                continue;
            }
            if best.map(|(_, b)| sim > b).unwrap_or(true) {
                best = Some((o_idx, sim));
            }
        }
        if let Some((o_idx, sim)) = best {
            let old_sym = old[o_idx].take().unwrap();
            let new_sym = new[n_idx].take().unwrap();
            leftover_old.retain(|&i| i != o_idx);
            pairings.push(Pairing::Rename {
                old: old_sym,
                new: new_sym,
                similarity: sim,
            });
        }
    }

    // Pass 3: anything left is one-sided.
    for slot in old.into_iter().flatten() {
        pairings.push(Pairing::OldOnly(slot));
    }
    for slot in new.into_iter().flatten() {
        pairings.push(Pairing::NewOnly(slot));
    }

    pairings
}

/// Compatibility filter for rename detection. Cross-kind renames (a
/// function turning into a class) are too risky to auto-detect; we
/// require the same `SymbolKind` and the same parent path (so we don't
/// promote a method to a free function or vice versa).
fn pairing_compatible(a: &Symbol, b: &Symbol) -> bool {
    if a.kind != b.kind {
        return false;
    }
    let a_parent = if a.path.is_empty() {
        &[][..]
    } else {
        &a.path[..a.path.len() - 1]
    };
    let b_parent = if b.path.is_empty() {
        &[][..]
    } else {
        &b.path[..b.path.len() - 1]
    };
    a_parent == b_parent
}

/// Normalized similarity between two signature strings in `0.0 ..= 1.0`.
/// Uses a token-set Jaccard with a small bias toward identical
/// signatures (so an exact-match signature change wins over a
/// half-match every time).
fn signature_similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let at = tokenize_signature(a);
    let bt = tokenize_signature(b);
    if at.is_empty() && bt.is_empty() {
        return 0.0;
    }
    let mut intersection = 0usize;
    for token in &at {
        if bt.contains(token) {
            intersection += 1;
        }
    }
    let union = at.len() + bt.len() - intersection;
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

/// Split a signature into significant tokens (alphanumeric sequences,
/// underscores grouped with the surrounding word). Drops single-char
/// punctuation. Lower-cases for case-insensitive matching.
fn tokenize_signature(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests;
