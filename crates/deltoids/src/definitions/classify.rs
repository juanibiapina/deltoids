//! Say what changed in each pair of definitions, and arrange the
//! definitions as a tree in the new file's order: the changes, and the
//! unchanged definitions around them as context.

use std::hash::{Hash, Hasher};
use std::ops::Range;

use rustc_hash::FxHasher;

use super::matching::{Link, Pairing, children};
use super::{Aspects, DefinitionChange, DefinitionDiff, Place, Status, TopLevelChange};
use crate::engine::DiffOp;
use crate::syntax::{Definition, Outline, TopStream};

/// Lines the line diff changed: removed old rows and added new rows; and
/// for each old row it kept, the new row it kept it at.
pub(super) struct ChangedRows {
    removed: Vec<bool>,
    added: Vec<bool>,
    pub(super) kept: Vec<Option<usize>>,
}

impl ChangedRows {
    pub(super) fn of(ops: &[DiffOp], old_rows: usize, new_rows: usize) -> Self {
        let mut rows = Self {
            removed: vec![false; old_rows],
            added: vec![false; new_rows],
            kept: vec![None; old_rows],
        };
        for op in ops {
            rows.mark(*op);
        }
        rows
    }

    fn mark(&mut self, op: DiffOp) {
        let set = |rows: &mut Vec<bool>, start: usize, len: usize| {
            rows.iter_mut()
                .skip(start)
                .take(len)
                .for_each(|slot| *slot = true);
        };
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                let rows = self.kept.iter_mut().skip(old_index).take(len);
                for (offset, slot) in rows.enumerate() {
                    *slot = Some(new_index + offset);
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => set(&mut self.added, new_index, new_len),
            DiffOp::Delete {
                old_index, old_len, ..
            } => set(&mut self.removed, old_index, old_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                set(&mut self.removed, old_index, old_len);
                set(&mut self.added, new_index, new_len);
            }
        }
    }
}

/// One version's definitions with what the line diff changed in each.
struct Side<'a> {
    outline: &'a Outline,
    children: Vec<Vec<usize>>,
    /// Per definition: changed own rows, the first of them, own non-blank
    /// rows, and how many of those changed.
    stats: Vec<Stats>,
    /// Per row: no definition holds it.
    free: Vec<bool>,
    /// Per row: code outside every definition is on it (no definition
    /// holds it, or a comment trails a definition's last line).
    top: Vec<bool>,
    /// Per row: it holds more than whitespace.
    filled: Vec<bool>,
}

#[derive(Default, Clone, Copy)]
struct Stats {
    changed: usize,
    first: Option<usize>,
    filled: usize,
    filled_changed: usize,
}

impl<'a> Side<'a> {
    fn new(outline: &'a Outline, changed: &[bool], text: &str) -> Self {
        let mut owner: Vec<Option<usize>> = vec![None; changed.len()];
        for (index, definition) in outline.definitions.iter().enumerate() {
            let rows = &definition.rows;
            owner
                .iter_mut()
                .skip(rows.start)
                .take(rows.len())
                .for_each(|slot| *slot = Some(index));
        }
        let mut stats = vec![Stats::default(); outline.definitions.len()];
        let mut filled = Vec::with_capacity(changed.len());
        for (row, line) in text.lines().enumerate() {
            let Some(&is_changed) = changed.get(row) else {
                break;
            };
            let has_text = !line.trim().is_empty();
            filled.push(has_text);
            let Some(index) = owner[row] else {
                continue;
            };
            let entry = &mut stats[index];
            if is_changed {
                entry.changed += 1;
                entry.first.get_or_insert(row + 1);
            }
            if has_text {
                entry.filled += 1;
                entry.filled_changed += usize::from(is_changed);
            }
        }
        let free: Vec<bool> = owner.iter().map(Option::is_none).collect();
        let mut top = free.clone();
        for token in &outline.top {
            let rows = token.rows.start.min(top.len())..token.rows.end.min(top.len());
            top[rows].fill(true);
        }
        Self {
            outline,
            children: children(outline),
            stats,
            free,
            top,
            filled,
        }
    }

    /// The rows among `rows` with code outside every definition on them.
    fn top_rows(&self, rows: Range<usize>) -> Vec<usize> {
        rows.filter(|&row| self.top.get(row).copied().unwrap_or(false))
            .collect()
    }

    /// Hashes of the comments, the imports, and the code on `rows`, sorted
    /// rows outside every definition, each in document order.
    fn top_hashes(&self, rows: &[usize]) -> [u64; 3] {
        let mut hashers: [FxHasher; 3] = Default::default();
        let (Some(&first), Some(&last)) = (rows.first(), rows.last()) else {
            return hashers.map(|hasher| hasher.finish());
        };
        let tokens = &self.outline.top;
        let start = tokens.partition_point(|token| token.rows.end <= first);
        let touching = tokens[start..]
            .iter()
            .take_while(|token| token.rows.start <= last)
            .filter(|token| {
                token
                    .rows
                    .clone()
                    .any(|row| rows.binary_search(&row).is_ok())
            });
        for token in touching {
            let slot = match token.stream {
                TopStream::Comments => 0,
                TopStream::Imports => 1,
                TopStream::Code => 2,
            };
            token.hash.hash(&mut hashers[slot]);
        }
        hashers.map(|hasher| hasher.finish())
    }

    fn definition(&self, index: usize) -> &'a Definition {
        &self.outline.definitions[index]
    }

    fn slot(&self, parent: Option<usize>) -> usize {
        parent.unwrap_or(self.outline.definitions.len())
    }

    fn place(&self, index: usize) -> Place {
        let definition = self.definition(index);
        Place {
            lines: definition.rows.start + 1..=definition.rows.end.max(definition.rows.start + 1),
            signature: definition.signature.clone(),
            first_change: self.stats[index].first,
        }
    }

    /// Names of `index`'s enclosing definitions, outermost first.
    fn path(&self, index: usize) -> Vec<String> {
        let mut names = Vec::new();
        let mut parent = self.definition(index).parent;
        while let Some(at) = parent {
            names.push(self.definition(at).name.clone());
            parent = self.definition(at).parent;
        }
        names.reverse();
        names
    }

    /// Every own non-blank line changed: the line diff removed or added it
    /// whole, as it does for a definition moved within its parent.
    fn rewritten(&self, index: usize) -> bool {
        let stats = self.stats[index];
        stats.filled > 0 && stats.filled_changed == stats.filled
    }
}

pub(super) fn diff(
    old: &Outline,
    new: &Outline,
    pairing: &Pairing,
    changed: &ChangedRows,
    ops: &[DiffOp],
    original: &str,
    updated: &str,
) -> DefinitionDiff {
    let old = Side::new(old, &changed.removed, original);
    let new = Side::new(new, &changed.added, updated);
    let builder = Builder {
        old: &old,
        new: &new,
        pairing,
    };
    let definitions = builder.under(None, None);
    DefinitionDiff {
        definitions,
        top_level: merge_top_level(&new, ops.iter().filter_map(|op| top_level(&old, &new, *op))),
        has_errors: old.outline.has_errors || new.outline.has_errors,
    }
}

/// One change per stretch of code between two definitions: changes with
/// no definition between them in the new file read as one. Each comes
/// with the new rows its line-diff operation covers.
fn merge_top_level(
    new: &Side,
    changes: impl Iterator<Item = (TopLevelChange, Range<usize>)>,
) -> Vec<TopLevelChange> {
    let mut merged: Vec<(TopLevelChange, Range<usize>)> = Vec::new();
    for (change, rows) in changes {
        let joins = merged.last().is_some_and(|(_, last)| {
            let between = last.end.min(rows.start)..rows.start;
            new.free[between].iter().all(|&free| free)
        });
        match merged.last_mut() {
            Some((last, span)) if joins => {
                last.aspects = last.aspects.union(change.aspects);
                last.lines = (last.lines.0 + change.lines.0, last.lines.1 + change.lines.1);
                last.new_first_change = last.new_first_change.or(change.new_first_change);
                last.old_first_change = last.old_first_change.or(change.old_first_change);
                span.end = rows.end;
            }
            _ => merged.push((change, rows)),
        }
    }
    merged.into_iter().map(|(change, _)| change).collect()
}

/// The change to the code outside every definition among the rows `op`
/// changed, when there is one: which of its comments, imports, and code
/// changed, and where it sits in the new file, with the new rows `op`
/// covers. Blank lines alone are no change.
fn top_level(old: &Side, new: &Side, op: DiffOp) -> Option<(TopLevelChange, Range<usize>)> {
    let (old_rows, new_rows, at) = match op {
        DiffOp::Equal { .. } => return None,
        DiffOp::Insert {
            old_index,
            new_index,
            new_len,
        } => (
            old_index..old_index,
            new_index..new_index + new_len,
            new_index,
        ),
        DiffOp::Delete {
            old_index,
            old_len,
            new_index,
        } => (
            old_index..old_index + old_len,
            new_index..new_index,
            new_index,
        ),
        DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } => (
            old_index..old_index + old_len,
            new_index..new_index + new_len,
            new_index,
        ),
    };
    let (old_top, new_top) = (old.top_rows(old_rows), new.top_rows(new_rows.clone()));
    let [old_comments, old_imports, old_code] = old.top_hashes(&old_top);
    let [new_comments, new_imports, new_code] = new.top_hashes(&new_top);
    let mut aspects = Aspects {
        comments: old_comments != new_comments,
        imports: old_imports != new_imports,
        body: old_code != new_code,
        ..Aspects::default()
    };
    // Only rows no definition holds can change their layout alone: on a
    // definition's row, the definition's own change explains the line.
    let laid_out =
        |side: &Side, rows: &[usize]| rows.iter().any(|&row| side.free[row] && side.filled[row]);
    aspects.formatting = aspects.is_empty() && (laid_out(old, &old_top) || laid_out(new, &new_top));
    let change = TopLevelChange {
        aspects,
        lines: (new_top.len(), old_top.len()),
        new_first_change: new_top.first().map(|row| row + 1),
        old_first_change: old_top.first().map(|row| row + 1),
        at: new_top.first().copied().unwrap_or(at) + 1,
    };
    (!change.aspects.is_empty()).then_some((change, new_rows))
}

struct Builder<'a> {
    old: &'a Side<'a>,
    new: &'a Side<'a>,
    pairing: &'a Pairing,
}

impl Builder<'_> {
    /// The children of an old and a new parent (the file for `None`), in
    /// new-file order with each removed definition after the one that
    /// preceded it in the old file.
    fn under(&self, old_parent: Option<usize>, new_parent: Option<usize>) -> Vec<DefinitionChange> {
        let new_children = &self.new.children[self.new.slot(new_parent)];
        let old_children = &self.old.children[self.old.slot(old_parent)];
        // Removed old children, attached after the new child that the
        // nearest preceding paired old sibling became (`None`: first).
        let mut removed_after: Vec<(Option<usize>, usize)> = Vec::new();
        let mut anchor: Option<usize> = None;
        for &o in old_children {
            match self.pairing.new_of[o] {
                Some(n) if new_children.contains(&n) => anchor = Some(n),
                Some(_) => {}
                None => removed_after.push((anchor, o)),
            }
        }
        let mut changes = Vec::new();
        let removed_at = |changes: &mut Vec<DefinitionChange>, at: Option<usize>| {
            for &(_, o) in removed_after.iter().filter(|(after, _)| *after == at) {
                changes.push(self.removed(o));
            }
        };
        removed_at(&mut changes, None);
        for &n in new_children {
            let change = match self.pairing.old_of[n] {
                None => self.added(n),
                Some((o, how)) => self.paired(o, n, how),
            };
            changes.push(change);
            removed_at(&mut changes, Some(n));
        }
        changes
    }

    fn added(&self, n: usize) -> DefinitionChange {
        let definition = self.new.definition(n);
        DefinitionChange {
            kind: definition.kind,
            name: definition.name.clone(),
            status: Status::Added,
            aspects: Aspects::default(),
            old: None,
            new: Some(self.new.place(n)),
            lines: (self.new.stats[n].changed, 0),
            // A child paired with an old definition moved in from elsewhere.
            children: self.new.children[n]
                .iter()
                .map(|&child| match self.pairing.old_of[child] {
                    None => self.added(child),
                    Some((o, how)) => self.paired(o, child, how),
                })
                .collect(),
            uncertain: definition.error,
        }
    }

    fn removed(&self, o: usize) -> DefinitionChange {
        let definition = self.old.definition(o);
        DefinitionChange {
            kind: definition.kind,
            name: definition.name.clone(),
            status: Status::Removed,
            aspects: Aspects::default(),
            old: Some(self.old.place(o)),
            new: None,
            lines: (0, self.old.stats[o].changed),
            // A child paired with a new definition moved; it shows there.
            children: self.old.children[o]
                .iter()
                .filter(|&&child| self.pairing.new_of[child].is_none())
                .map(|&child| self.removed(child))
                .collect(),
            uncertain: definition.error,
        }
    }

    fn paired(&self, o: usize, n: usize, how: Link) -> DefinitionChange {
        let (before, after) = (self.old.definition(o), self.new.definition(n));
        let (a, b) = (&before.content, &after.content);
        let signature = match how {
            Link::Renamed => a.unnamed_signature != b.unnamed_signature,
            Link::Same | Link::Moved => a.signature != b.signature,
        };
        let mut aspects = Aspects {
            signature,
            body: a.body != b.body,
            docs: a.docs != b.docs,
            comments: a.comments != b.comments,
            attributes: a.attributes != b.attributes,
            ..Aspects::default()
        };
        aspects.formatting = how != Link::Renamed && aspects.is_empty() && a.raw != b.raw;
        let mut children = self.under(Some(o), Some(n));
        let status = match how {
            Link::Renamed => Status::Renamed {
                from: before.name.clone(),
            },
            Link::Moved => Status::Moved {
                from: Some(self.old.path(o)),
            },
            Link::Same if !aspects.is_empty() => Status::Modified,
            Link::Same if self.old.rewritten(o) && self.new.rewritten(n) => {
                Status::Moved { from: None }
            }
            Link::Same if children.iter().any(DefinitionChange::is_changed) => Status::Container,
            Link::Same => Status::Unchanged,
        };
        // Everything inside an unchanged definition is unchanged too.
        if status == Status::Unchanged {
            children.clear();
        }
        DefinitionChange {
            kind: after.kind,
            name: after.name.clone(),
            status,
            aspects,
            old: Some(self.old.place(o)),
            new: Some(self.new.place(n)),
            lines: (self.new.stats[n].changed, self.old.stats[o].changed),
            children,
            uncertain: before.error || after.error,
        }
    }
}
