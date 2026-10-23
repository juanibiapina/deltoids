//! Say what changed in each pair of definitions, and arrange the changes
//! as a tree in the new file's order.

use super::matching::{Link, Pairing, children};
use super::{Aspects, DefinitionChange, DefinitionDiff, Place, Status, TopLevelChange};
use crate::engine::DiffOp;
use crate::syntax::{Definition, Outline};

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
    top: Stats,
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
        let mut top = Stats::default();
        for (row, line) in text.lines().enumerate() {
            let Some(&is_changed) = changed.get(row) else {
                break;
            };
            let entry = match owner[row] {
                Some(index) => &mut stats[index],
                None => &mut top,
            };
            if is_changed {
                entry.changed += 1;
                entry.first.get_or_insert(row + 1);
            }
            if !line.trim().is_empty() {
                entry.filled += 1;
                entry.filled_changed += usize::from(is_changed);
            }
        }
        Self {
            outline,
            children: children(outline),
            stats,
            top,
        }
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
    let changes = builder.under(None, None);
    DefinitionDiff {
        changes,
        top_level: top_level(&old, &new),
        has_errors: old.outline.has_errors || new.outline.has_errors,
    }
}

fn top_level(old: &Side, new: &Side) -> Option<TopLevelChange> {
    let (a, b) = (&old.outline.top, &new.outline.top);
    let mut aspects = Aspects {
        body: a.body != b.body,
        comments: a.comments != b.comments,
        imports: a.imports != b.imports,
        ..Aspects::default()
    };
    // Code that only moved past definitions (an import moved to the end)
    // hashes the same; the lines the diff changed still show it.
    let moved = old.top.filled_changed + new.top.filled_changed > 0;
    aspects.formatting = aspects.is_empty() && (a.raw != b.raw || moved);
    (!aspects.is_empty()).then_some(TopLevelChange {
        aspects,
        lines: (new.top.changed, old.top.changed),
        new_first_change: new.top.first,
        old_first_change: old.top.first,
    })
}

struct Builder<'a> {
    old: &'a Side<'a>,
    new: &'a Side<'a>,
    pairing: &'a Pairing,
}

impl Builder<'_> {
    /// The changes among the children of an old and a new parent (the
    /// file for `None`), in new-file order with each removed definition
    /// after the one that preceded it in the old file.
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
                None => Some(self.added(n)),
                Some((o, how)) => self.paired(o, n, how),
            };
            changes.extend(change);
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
                .filter_map(|&child| match self.pairing.old_of[child] {
                    None => Some(self.added(child)),
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

    fn paired(&self, o: usize, n: usize, how: Link) -> Option<DefinitionChange> {
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
        let children = self.under(Some(o), Some(n));
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
            Link::Same if !children.is_empty() => Status::Container,
            Link::Same => return None,
        };
        Some(DefinitionChange {
            kind: after.kind,
            name: after.name.clone(),
            status,
            aspects,
            old: Some(self.old.place(o)),
            new: Some(self.new.place(n)),
            lines: (self.new.stats[n].changed, self.old.stats[o].changed),
            children,
            uncertain: before.error || after.error,
        })
    }
}
