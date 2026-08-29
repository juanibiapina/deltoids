//! Display layout for a change run's removed/added lines.
//!
//! A change run (the `Removed`/`Added` slice of a [`crate::HunkRun::Change`])
//! is stored grouped: all removed lines, then all added lines. Some readers
//! prefer them interleaved, so a removed line sits next to its added
//! counterpart. [`ChangeLayout`] selects the layout and [`arrange_change`]
//! reorders one run accordingly.
//!
//! The reordering is display-only: it never drops, invents, or reclassifies a
//! line, and it preserves the relative order *within* each kind. Callers that
//! carry per-line state (intraline emphasis) bind that state to each line
//! before calling [`arrange_change`], so the reordering cannot desynchronise
//! it.

use std::num::NonZeroUsize;

use crate::LineKind;

/// How a change run lays out its removed/added lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ChangeLayout {
    /// Stored order: all removed lines, then all added lines. This is the
    /// default and reproduces the engine's stored order unchanged.
    #[default]
    Grouped,
    /// Alternate blocks of up to `group` removed then `group` added lines.
    /// `group == 1` is fully interleaved (`-+-+`).
    Interleaved { group: NonZeroUsize },
}

/// Reorder one change run for display.
///
/// `items` is a change run (only `Removed`/`Added` items; `kind` reads each
/// item's [`LineKind`]). Returns borrowed references into `items` in display
/// order. The relative order within removed items and within added items is
/// always preserved; only their interleaving changes.
///
/// - [`ChangeLayout::Grouped`] returns `items` in stored order (identity).
/// - [`ChangeLayout::Interleaved`] emits alternating chunks of up to `group`
///   removed then `group` added items; when one kind outnumbers the other its
///   remainder trails at the end.
pub fn arrange_change<T>(
    items: &[T],
    kind: impl Fn(&T) -> LineKind,
    layout: ChangeLayout,
) -> Vec<&T> {
    let group = match layout {
        ChangeLayout::Grouped => return items.iter().collect(),
        ChangeLayout::Interleaved { group } => group.get(),
    };

    let removed: Vec<&T> = items
        .iter()
        .filter(|item| kind(item) == LineKind::Removed)
        .collect();
    let added: Vec<&T> = items
        .iter()
        .filter(|item| kind(item) == LineKind::Added)
        .collect();

    let mut out = Vec::with_capacity(items.len());
    let mut ri = 0;
    let mut ai = 0;
    while ri < removed.len() || ai < added.len() {
        let r_end = (ri + group).min(removed.len());
        out.extend(removed[ri..r_end].iter().copied());
        ri = r_end;

        let a_end = (ai + group).min(added.len());
        out.extend(added[ai..a_end].iter().copied());
        ai = a_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    /// A change run as `(kind, content)` pairs, with `R`/`A` shorthands.
    fn r(content: &str) -> (LineKind, &str) {
        (LineKind::Removed, content)
    }
    fn a(content: &str) -> (LineKind, &str) {
        (LineKind::Added, content)
    }

    fn arrange<'a>(items: &'a [(LineKind, &'a str)], layout: ChangeLayout) -> Vec<&'a str> {
        arrange_change(items, |(k, _)| k.clone(), layout)
            .into_iter()
            .map(|(_, c)| *c)
            .collect()
    }

    #[test]
    fn grouped_is_identity() {
        let items = [r("r1"), r("r2"), a("a1"), a("a2")];
        assert_eq!(
            arrange(&items, ChangeLayout::Grouped),
            vec!["r1", "r2", "a1", "a2"]
        );
    }

    #[test]
    fn grouped_preserves_unusual_stored_order() {
        // Grouped never re-canonicalises: an added-first run stays as stored.
        let items = [a("a1"), r("r1")];
        assert_eq!(arrange(&items, ChangeLayout::Grouped), vec!["a1", "r1"]);
    }

    #[test]
    fn interleaved_group_1_alternates() {
        let items = [r("r1"), r("r2"), r("r3"), a("a1"), a("a2"), a("a3")];
        assert_eq!(
            arrange(&items, ChangeLayout::Interleaved { group: nz(1) }),
            vec!["r1", "a1", "r2", "a2", "r3", "a3"]
        );
    }

    #[test]
    fn interleaved_group_2_chunks() {
        let items = [
            r("r1"),
            r("r2"),
            r("r3"),
            r("r4"),
            a("a1"),
            a("a2"),
            a("a3"),
            a("a4"),
        ];
        assert_eq!(
            arrange(&items, ChangeLayout::Interleaved { group: nz(2) }),
            vec!["r1", "r2", "a1", "a2", "r3", "r4", "a3", "a4"]
        );
    }

    #[test]
    fn interleaved_unequal_counts_spill_remainder() {
        // 3 removed, 5 added at group 1: pairs first, added remainder trails.
        let items = [
            r("r1"),
            r("r2"),
            r("r3"),
            a("a1"),
            a("a2"),
            a("a3"),
            a("a4"),
            a("a5"),
        ];
        assert_eq!(
            arrange(&items, ChangeLayout::Interleaved { group: nz(1) }),
            vec!["r1", "a1", "r2", "a2", "r3", "a3", "a4", "a5"]
        );
    }

    #[test]
    fn interleaved_pure_insert_unchanged() {
        let items = [a("a1"), a("a2")];
        assert_eq!(
            arrange(&items, ChangeLayout::Interleaved { group: nz(1) }),
            vec!["a1", "a2"]
        );
    }

    #[test]
    fn interleaved_pure_delete_unchanged() {
        let items = [r("r1"), r("r2")];
        assert_eq!(
            arrange(&items, ChangeLayout::Interleaved { group: nz(1) }),
            vec!["r1", "r2"]
        );
    }

    #[test]
    fn every_layout_preserves_multiset_and_intra_kind_order() {
        let items = [r("r1"), r("r2"), r("r3"), a("a1"), a("a2")];
        for layout in [
            ChangeLayout::Grouped,
            ChangeLayout::Interleaved { group: nz(1) },
            ChangeLayout::Interleaved { group: nz(2) },
            ChangeLayout::Interleaved { group: nz(9) },
        ] {
            let out = arrange(&items, layout);
            // Same multiset of lines.
            let mut sorted = out.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, vec!["a1", "a2", "r1", "r2", "r3"]);
            // Relative order within each kind preserved.
            let removed: Vec<&str> = out.iter().copied().filter(|c| c.starts_with('r')).collect();
            let added: Vec<&str> = out.iter().copied().filter(|c| c.starts_with('a')).collect();
            assert_eq!(removed, vec!["r1", "r2", "r3"]);
            assert_eq!(added, vec!["a1", "a2"]);
        }
    }
}
