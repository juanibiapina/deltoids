//! Diff engine: thin layer over `gix-imara-diff`.
//!
//! Public surface for deltoids consumers is [`Snapshot`] (and the
//! `align_old_to_new` method on it). [`DiffOp`] is re-exported from
//! `lib.rs` for callers that want to walk the raw operation stream
//! (e.g. to bypass tree-sitter scope expansion).
//!
//! Internal to deltoids: the free [`align_old_to_new`] function, which
//! the scope planner uses with hand-built `Vec<DiffOp>` test fixtures
//! without constructing a [`Snapshot`].

use gix_imara_diff::{
    Algorithm, BasicLineDiffPrinter, Diff as ImaraDiff, InternedInput, UnifiedDiffConfig,
};

// ---------------------------------------------------------------------------
// DiffOp
// ---------------------------------------------------------------------------

/// One operation in a line-level diff.
///
/// Mirrors the four cases produced by `gix_imara_diff::Diff::hunks()`
/// plus synthesized `Equal` gaps between hunks. Indices are 0-based
/// line numbers; `len`/`old_len`/`new_len` are line counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp {
    Equal {
        old_index: usize,
        new_index: usize,
        len: usize,
    },
    Insert {
        old_index: usize,
        new_index: usize,
        new_len: usize,
    },
    Delete {
        old_index: usize,
        old_len: usize,
        new_index: usize,
    },
    Replace {
        old_index: usize,
        old_len: usize,
        new_index: usize,
        new_len: usize,
    },
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Eager, owned snapshot of a line-level diff between two strings.
///
/// Computed once via [`Snapshot::compute`]: stores the full op stream
/// and the unified diff text (with the standard 3-line context). Drops
/// the imara types after construction so the snapshot has no lifetime
/// parameter.
#[derive(Debug, Clone)]
pub struct Snapshot {
    ops: Vec<DiffOp>,
    unified_text: String,
}

impl Snapshot {
    /// Compute the line-level diff between `original` and `updated`
    /// using the Histogram algorithm with imara's line postprocessing.
    pub fn compute(original: &str, updated: &str) -> Self {
        let input = InternedInput::new(original, updated);
        let mut diff = ImaraDiff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);

        let total_old = original.lines().count();
        let total_new = updated.lines().count();
        let ops = ops_from_imara(&diff, total_old, total_new);
        let unified_text = unified_diff_text(&diff, &input);

        Snapshot { ops, unified_text }
    }

    /// The full diff op stream, including synthesized `Equal` gaps.
    pub fn ops(&self) -> &[DiffOp] {
        &self.ops
    }

    /// Unified diff text with the standard 3-line context, prefixed
    /// with `--- original` / `+++ modified` headers. Empty string when
    /// the inputs are identical.
    pub fn unified_text(&self) -> &str {
        &self.unified_text
    }

    /// Map an OLD-file 0-indexed line to its NEW-file equivalent
    /// through this snapshot's op stream. See [`align_old_to_new`].
    pub fn align_old_to_new(&self, line: usize) -> Option<usize> {
        align_old_to_new(line, &self.ops)
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Map an OLD-file 0-indexed line to its NEW-file equivalent through
/// the diff ops.
///
/// - Lines in an `Equal` op return their aligned new position.
/// - Lines in a `Replace` op return the closest line in the new range,
///   clamped to the last line of the new range. The clamp matters for
///   `scope.end` of multi-line replaces so `}` maps to `}` rather than
///   to the start of the new range.
/// - Lines in a `Delete` op return `None` (no counterpart in NEW).
/// - Lines outside any op return `None`.
pub fn align_old_to_new(line: usize, ops: &[DiffOp]) -> Option<usize> {
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                if line >= *old_index && line < old_index + len {
                    return Some(new_index + (line - old_index));
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                if line >= *old_index && line < old_index + old_len {
                    let local = line - old_index;
                    let clamped = local.min(new_len.saturating_sub(1));
                    return Some(new_index + clamped);
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                if line >= *old_index && line < old_index + old_len {
                    return None;
                }
            }
            DiffOp::Insert { .. } => {}
        }
    }
    None
}

/// Build a `Vec<DiffOp>` from a `gix_imara_diff::Diff`, synthesizing
/// `Equal` gaps between consecutive hunks (and at the head/tail of the
/// file).
fn ops_from_imara(diff: &ImaraDiff, total_old: usize, total_new: usize) -> Vec<DiffOp> {
    let mut ops = Vec::new();
    let mut old_cursor: usize = 0;
    let mut new_cursor: usize = 0;

    for hunk in diff.hunks() {
        let before_start = hunk.before.start as usize;
        let after_start = hunk.after.start as usize;
        let before_len = (hunk.before.end - hunk.before.start) as usize;
        let after_len = (hunk.after.end - hunk.after.start) as usize;

        if before_start > old_cursor {
            let len = before_start - old_cursor;
            // Defensive: gap on old side must equal gap on new side for an
            // Equal stretch. gix-imara-diff guarantees this.
            debug_assert_eq!(len, after_start - new_cursor);
            ops.push(DiffOp::Equal {
                old_index: old_cursor,
                new_index: new_cursor,
                len,
            });
        }

        if before_len == 0 && after_len > 0 {
            ops.push(DiffOp::Insert {
                old_index: before_start,
                new_index: after_start,
                new_len: after_len,
            });
        } else if before_len > 0 && after_len == 0 {
            ops.push(DiffOp::Delete {
                old_index: before_start,
                old_len: before_len,
                new_index: after_start,
            });
        } else if before_len > 0 && after_len > 0 {
            ops.push(DiffOp::Replace {
                old_index: before_start,
                old_len: before_len,
                new_index: after_start,
                new_len: after_len,
            });
        }

        old_cursor = before_start + before_len;
        new_cursor = after_start + after_len;
    }

    // Trailing equal stretch.
    if old_cursor < total_old {
        let len = total_old - old_cursor;
        debug_assert_eq!(len, total_new - new_cursor);
        ops.push(DiffOp::Equal {
            old_index: old_cursor,
            new_index: new_cursor,
            len,
        });
    }

    ops
}

/// Render the unified diff text with imara's basic printer, prefixed
/// with `--- original` / `+++ modified` headers. Empty string when the
/// inputs are identical.
fn unified_diff_text(diff: &ImaraDiff, input: &InternedInput<&str>) -> String {
    let printer = BasicLineDiffPrinter(&input.interner);
    let body = diff
        .unified_diff(&printer, UnifiedDiffConfig::default(), input)
        .to_string();
    if body.is_empty() {
        String::new()
    } else {
        let mut out = String::with_capacity(body.len() + 32);
        out.push_str("--- original\n+++ modified\n");
        out.push_str(&body);
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Snapshot::compute smoke test
    // -----------------------------------------------------------------------

    #[test]
    fn compute_produces_ops_and_unified_text_for_one_line_change() {
        let snap = Snapshot::compute("a\nb\n", "a\nB\n");

        // Op stream has at least one non-Equal op.
        assert!(
            snap.ops()
                .iter()
                .any(|op| !matches!(op, DiffOp::Equal { .. })),
            "expected at least one change op, got {:?}",
            snap.ops()
        );

        // Unified text contains the change and the standard headers.
        let text = snap.unified_text();
        assert!(text.contains("--- original"), "missing header in {text:?}");
        assert!(text.contains("+++ modified"), "missing header in {text:?}");
        assert!(text.contains("-b"), "missing removed line in {text:?}");
        assert!(text.contains("+B"), "missing added line in {text:?}");
    }

    #[test]
    fn compute_empty_inputs_produces_empty_ops_and_empty_text() {
        let snap = Snapshot::compute("", "");
        assert!(snap.ops().is_empty());
        assert!(snap.unified_text().is_empty());
    }

    #[test]
    fn compute_identical_inputs_produces_only_equal_ops() {
        let snap = Snapshot::compute("a\nb\nc\n", "a\nb\nc\n");
        assert!(
            snap.ops()
                .iter()
                .all(|op| matches!(op, DiffOp::Equal { .. }))
        );
        assert!(snap.unified_text().is_empty());
    }

    // -----------------------------------------------------------------------
    // align_old_to_new unit tests
    //
    // Pin the contract per `DiffOp` variant. These tests use hand-built
    // op fixtures so they do not depend on the imara algorithm picking
    // a particular shape.
    // -----------------------------------------------------------------------

    #[test]
    fn align_old_to_new_equal_op_returns_aligned_line() {
        // Equal { old=5..10, new=8..13 }: line 5 -> 8, 7 -> 10, 9 -> 12.
        let ops = vec![DiffOp::Equal {
            old_index: 5,
            new_index: 8,
            len: 5,
        }];
        assert_eq!(align_old_to_new(5, &ops), Some(8));
        assert_eq!(align_old_to_new(7, &ops), Some(10));
        assert_eq!(align_old_to_new(9, &ops), Some(12));
        // Line outside the equal range has no mapping.
        assert_eq!(align_old_to_new(10, &ops), None);
    }

    #[test]
    fn align_old_to_new_replace_op_clamps_to_last_new_line() {
        // Replace { old=10..15 (5 lines), new=20..23 (3 lines) }.
        // Inside the replace, lines map to ni + min(local_offset, new_len-1).
        // This keeps the LAST old line mapped to the LAST new line, which
        // is what `same_slot` needs for `scope.end` of asymmetric replaces
        // (e.g. `};` -> `}` where the closing brace IS the replace).
        let ops = vec![DiffOp::Replace {
            old_index: 10,
            old_len: 5,
            new_index: 20,
            new_len: 3,
        }];
        assert_eq!(align_old_to_new(10, &ops), Some(20)); // first -> first
        assert_eq!(align_old_to_new(11, &ops), Some(21));
        assert_eq!(align_old_to_new(12, &ops), Some(22)); // clamped
        assert_eq!(align_old_to_new(13, &ops), Some(22)); // clamped
        assert_eq!(align_old_to_new(14, &ops), Some(22)); // last old -> last new
    }

    #[test]
    fn align_old_to_new_delete_op_returns_none() {
        // Delete { old=4..7 }: lines 4..7 have no NEW counterpart.
        let ops = vec![DiffOp::Delete {
            old_index: 4,
            old_len: 3,
            new_index: 4,
        }];
        assert_eq!(align_old_to_new(4, &ops), None);
        assert_eq!(align_old_to_new(5, &ops), None);
        assert_eq!(align_old_to_new(6, &ops), None);
    }

    #[test]
    fn align_old_to_new_chain_with_insert_keeps_alignment() {
        // Realistic chain: Equal -> Insert (no OLD lines) -> Equal.
        // The insert shifts NEW indices but does not consume OLD indices,
        // so OLD lines after the insert map cleanly via the second Equal
        // op (whose new_index already accounts for the shift).
        let ops = vec![
            DiffOp::Equal {
                old_index: 0,
                new_index: 0,
                len: 3,
            },
            DiffOp::Insert {
                old_index: 3,
                new_index: 3,
                new_len: 2,
            },
            DiffOp::Equal {
                old_index: 3,
                new_index: 5,
                len: 4,
            },
        ];
        // Lines 0..3 map identity (first Equal).
        assert_eq!(align_old_to_new(0, &ops), Some(0));
        assert_eq!(align_old_to_new(2, &ops), Some(2));
        // Lines 3..7 map +2 (after the 2-line Insert).
        assert_eq!(align_old_to_new(3, &ops), Some(5));
        assert_eq!(align_old_to_new(4, &ops), Some(6));
        assert_eq!(align_old_to_new(6, &ops), Some(8));
        // Beyond the last op: no mapping.
        assert_eq!(align_old_to_new(7, &ops), None);
    }

    #[test]
    fn snapshot_align_old_to_new_delegates_to_free_function() {
        // Insert at line 1 -> NEW line 1 has no OLD counterpart, but
        // OLD line 0 still maps to NEW 0, and OLD line 1 (which becomes
        // NEW 2 after the insert) is reachable via the trailing Equal op
        // synthesized by `ops_from_imara`.
        let snap = Snapshot::compute("a\nb\n", "a\nINSERTED\nb\n");
        assert_eq!(snap.align_old_to_new(0), Some(0));
        // OLD `b` (line 1) shifts to NEW line 2 after the insert.
        assert_eq!(snap.align_old_to_new(1), Some(2));
    }
}
