//! Line-level diff engine: a thin layer over `gix-imara-diff`.
//!
//! [`Snapshot::compute`] runs the histogram diff once and keeps the op
//! stream (for hunk construction in [`crate::scope`]) and the standard
//! unified text (for [`crate::Diff::text`]).

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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
}
