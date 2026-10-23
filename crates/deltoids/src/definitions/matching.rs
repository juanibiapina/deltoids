//! Pair the definitions of an old and a new outline.
//!
//! Pairs form level by level, starting at the file: children of a pair
//! pair among themselves. Within a parent, definitions with the same name
//! and family pair first (identical ones before the rest, then in order);
//! the leftovers pair as renames when their content, name aside, matches
//! exactly or nearly. Definitions still unpaired then pair across parents
//! as moves when their name and family are unique among the unpaired on
//! both sides.

use rustc_hash::FxHashMap;

use super::Family;
use crate::syntax::{Definition, Outline};

/// Definitions of each side, grouped by family and name.
type Groups<'a> = FxHashMap<(Family, &'a str), (Vec<usize>, Vec<usize>)>;

/// Least token similarity (Dice coefficient) for a rename.
const RENAME_SIMILARITY: f64 = 0.8;
/// Fewest tokens either side needs before similarity counts.
const RENAME_MIN_TOKENS: usize = 12;
/// Most candidate pairs compared for similarity under one parent.
const SIMILARITY_BUDGET: usize = 10_000;

/// How a new definition pairs with an old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Link {
    Same,
    Renamed,
    Moved,
}

/// The pairs: `old_of[new]` is the old definition a new one pairs with.
pub(super) struct Pairing {
    pub(super) old_of: Vec<Option<(usize, Link)>>,
    pub(super) new_of: Vec<Option<usize>>,
}

/// Children of each definition, and of the file in the last slot.
pub(super) fn children(outline: &Outline) -> Vec<Vec<usize>> {
    let root = outline.definitions.len();
    let mut children = vec![Vec::new(); root + 1];
    for (index, definition) in outline.definitions.iter().enumerate() {
        children[definition.parent.unwrap_or(root)].push(index);
    }
    children
}

/// Pair the definitions of `old` and `new`. `kept[row]` is the new row
/// the line diff kept old `row` at, if it kept it: among definitions
/// that could pair, one whose first line the line diff kept in place pairs
/// first, so the pairs agree with the diff's lines.
pub(super) fn pair(old: &Outline, new: &Outline, kept: &[Option<usize>]) -> Pairing {
    let mut pairing = Pairing {
        old_of: vec![None; new.definitions.len()],
        new_of: vec![None; old.definitions.len()],
    };
    let old_children = children(old);
    let new_children = children(new);
    let mut levels = vec![(old.definitions.len(), new.definitions.len())];
    let mut pending = Vec::new();
    while let Some((old_slot, new_slot)) = levels.pop() {
        pair_children(
            Sides { old, new, kept },
            &old_children[old_slot],
            &new_children[new_slot],
            &mut pairing,
            &mut pending,
        );
        levels.append(&mut pending);
    }
    pair_moves(
        Sides { old, new, kept },
        &mut pairing,
        &old_children,
        &new_children,
    );
    pairing
}

/// Both outlines, and the rows the line diff kept.
#[derive(Clone, Copy)]
struct Sides<'a> {
    old: &'a Outline,
    new: &'a Outline,
    kept: &'a [Option<usize>],
}

impl Sides<'_> {
    /// The line diff kept the line `o`'s own syntax starts on at the line
    /// `n`'s starts on.
    fn kept_in_place(&self, o: usize, n: usize) -> bool {
        let (before, after) = (&self.old.definitions[o], &self.new.definitions[n]);
        self.kept.get(before.head_row).copied().flatten() == Some(after.head_row)
    }
}

fn key(definition: &Definition) -> (Family, &str) {
    (definition.kind.family(), definition.name.as_str())
}

/// The content of a definition except its whitespace.
fn same_content(old: &Definition, new: &Definition) -> bool {
    let (a, b) = (&old.content, &new.content);
    a.signature == b.signature
        && a.body == b.body
        && a.comments == b.comments
        && a.docs == b.docs
        && a.attributes == b.attributes
}

fn link(
    pairing: &mut Pairing,
    old: usize,
    new: usize,
    how: Link,
    pending: &mut Vec<(usize, usize)>,
) {
    pairing.old_of[new] = Some((old, how));
    pairing.new_of[old] = Some(new);
    pending.push((old, new));
}

fn pair_children(
    sides: Sides,
    old_children: &[usize],
    new_children: &[usize],
    pairing: &mut Pairing,
    pending: &mut Vec<(usize, usize)>,
) {
    let (old, new) = (sides.old, sides.new);
    let mut by_key: Groups = FxHashMap::default();
    for &index in old_children {
        by_key
            .entry(key(&old.definitions[index]))
            .or_default()
            .0
            .push(index);
    }
    for &index in new_children {
        by_key
            .entry(key(&new.definitions[index]))
            .or_default()
            .1
            .push(index);
    }
    let mut keys: Vec<_> = by_key.into_iter().collect();
    keys.sort_by_key(|(_, (olds, news))| (news.first().copied(), olds.first().copied()));
    for (_, (olds, news)) in keys {
        pair_same_key(sides, &olds, &news, pairing, pending);
    }
    pair_renames(old, new, old_children, new_children, pairing, pending);
}

/// Pair definitions that share a family and a name: one the line diff
/// kept in place first, then identical ones, then the rest in order.
fn pair_same_key(
    sides: Sides,
    olds: &[usize],
    news: &[usize],
    pairing: &mut Pairing,
    pending: &mut Vec<(usize, usize)>,
) {
    let mut taken = vec![false; olds.len()];
    let take = |taken: &mut Vec<bool>, wanted: &dyn Fn(usize) -> bool| {
        let found = olds
            .iter()
            .copied()
            .enumerate()
            .find(|&(i, o)| !taken[i] && wanted(o));
        found.map(|(i, o)| {
            taken[i] = true;
            o
        })
    };
    let mut unkept: Vec<usize> = Vec::new();
    for &n in news {
        match take(&mut taken, &|o| sides.kept_in_place(o, n)) {
            Some(o) => link(pairing, o, n, Link::Same, pending),
            None => unkept.push(n),
        }
    }
    let mut open: Vec<usize> = Vec::new();
    for n in unkept {
        let identical =
            |o: usize| same_content(&sides.old.definitions[o], &sides.new.definitions[n]);
        match take(&mut taken, &identical) {
            Some(o) => link(pairing, o, n, Link::Same, pending),
            None => open.push(n),
        }
    }
    let rest = olds
        .iter()
        .zip(taken)
        .filter(|(_, taken)| !taken)
        .map(|(&o, _)| o);
    for (o, n) in rest.zip(open) {
        link(pairing, o, n, Link::Same, pending);
    }
}

fn pair_renames(
    old: &Outline,
    new: &Outline,
    old_children: &[usize],
    new_children: &[usize],
    pairing: &mut Pairing,
    pending: &mut Vec<(usize, usize)>,
) {
    let olds: Vec<usize> = old_children
        .iter()
        .copied()
        .filter(|&o| pairing.new_of[o].is_none())
        .collect();
    let news: Vec<usize> = new_children
        .iter()
        .copied()
        .filter(|&n| pairing.old_of[n].is_none())
        .collect();
    if olds.is_empty() || news.is_empty() {
        return;
    }
    // Exact: everything but the name matches, with one candidate a side.
    let exact = |o: usize, n: usize| {
        let (a, b) = (&old.definitions[o], &new.definitions[n]);
        a.kind.family() == b.kind.family()
            && a.content.unnamed_signature == b.content.unnamed_signature
            && a.content.body == b.content.body
    };
    for &o in &olds {
        let candidates: Vec<usize> = news
            .iter()
            .copied()
            .filter(|&n| pairing.old_of[n].is_none() && exact(o, n))
            .collect();
        if let [n] = candidates[..] {
            let rivals = olds
                .iter()
                .filter(|&&other| pairing.new_of[other].is_none() && exact(other, n))
                .count();
            if rivals == 1 {
                link(pairing, o, n, Link::Renamed, pending);
            }
        }
    }
    // Near: mutual best token similarity.
    let olds: Vec<usize> = olds
        .into_iter()
        .filter(|&o| pairing.new_of[o].is_none())
        .collect();
    let news: Vec<usize> = news
        .into_iter()
        .filter(|&n| pairing.old_of[n].is_none())
        .collect();
    if olds.len() * news.len() > SIMILARITY_BUDGET {
        return;
    }
    let sorted = |definition: &Definition| {
        let mut tokens = definition.content.tokens.clone();
        tokens.sort_unstable();
        tokens
    };
    let old_tokens: Vec<Vec<u64>> = olds.iter().map(|&o| sorted(&old.definitions[o])).collect();
    let new_tokens: Vec<Vec<u64>> = news.iter().map(|&n| sorted(&new.definitions[n])).collect();
    let mut scores = vec![vec![0.0; news.len()]; olds.len()];
    for (i, &o) in olds.iter().enumerate() {
        for (j, &n) in news.iter().enumerate() {
            let (a, b) = (&old_tokens[i], &new_tokens[j]);
            if old.definitions[o].kind.family() == new.definitions[n].kind.family()
                && a.len() >= RENAME_MIN_TOKENS
                && b.len() >= RENAME_MIN_TOKENS
            {
                scores[i][j] = dice(a, b);
            }
        }
    }
    for (i, &o) in olds.iter().enumerate() {
        let Some(j) = best(scores[i].iter().copied()) else {
            continue;
        };
        let column = scores.iter().map(|row| row[j]);
        if scores[i][j] >= RENAME_SIMILARITY && best(column) == Some(i) {
            link(pairing, o, news[j], Link::Renamed, pending);
        }
    }
}

/// Index of the single highest score; `None` on a tie or no positive score.
fn best(scores: impl Iterator<Item = f64>) -> Option<usize> {
    let mut top: Option<(usize, f64)> = None;
    let mut tied = false;
    for (index, score) in scores.enumerate() {
        match top {
            Some((_, high)) if score < high => {}
            Some((_, high)) if score == high => tied = true,
            _ if score > 0.0 => {
                top = Some((index, score));
                tied = false;
            }
            _ => {}
        }
    }
    top.filter(|_| !tied).map(|(index, _)| index)
}

/// Dice coefficient of two sorted token multisets.
fn dice(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    (2 * common) as f64 / (a.len() + b.len()) as f64
}

/// Pair unpaired definitions across parents by a name and family unique
/// among the unpaired on both sides, then pair their children.
fn pair_moves(
    sides: Sides,
    pairing: &mut Pairing,
    old_children: &[Vec<usize>],
    new_children: &[Vec<usize>],
) {
    let (old, new) = (sides.old, sides.new);
    let mut counts: Groups = FxHashMap::default();
    for (index, definition) in old.definitions.iter().enumerate() {
        if pairing.new_of[index].is_none() {
            counts.entry(key(definition)).or_default().0.push(index);
        }
    }
    for (index, definition) in new.definitions.iter().enumerate() {
        if pairing.old_of[index].is_none() {
            counts.entry(key(definition)).or_default().1.push(index);
        }
    }
    let mut moves: Vec<(usize, usize)> = counts
        .into_values()
        .filter_map(|(olds, news)| match (&olds[..], &news[..]) {
            ([o], [n]) => Some((*o, *n)),
            _ => None,
        })
        .collect();
    moves.sort_unstable();
    let mut pending = Vec::new();
    for (o, n) in moves {
        if pairing.new_of[o].is_none() && pairing.old_of[n].is_none() {
            link(pairing, o, n, Link::Moved, &mut pending);
        }
    }
    while let Some((o, n)) = pending.pop() {
        let mut more = Vec::new();
        pair_children(
            sides,
            &old_children[o],
            &new_children[n],
            pairing,
            &mut more,
        );
        pending.append(&mut more);
    }
}
