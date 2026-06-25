//! Apply axis: the edit operations and the splice engine. Validates every
//! anchor against the original file before mutating anything, lowers each
//! op to a concrete line span, checks for overlaps, then applies the batch
//! bottom-up.

use std::fmt::Write as _;

use super::anchor::{Anchor, AnchorOrBoundary, BODY_SEP, InsertSide, compute_line_hash};

/// Sentinel hash used by range edits to indicate "don't validate the
/// interior; only the first and last anchors are checked".
const RANGE_INTERIOR_HASH: &str = "**";

/// One hashline edit operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashEdit {
    /// Replace the inclusive range `[pos, end]` (where `end` defaults to
    /// `pos`) with `lines`. An empty `lines` deletes the range.
    Replace {
        reason: String,
        pos: Anchor,
        end: Option<Anchor>,
        lines: Vec<String>,
    },
    /// Insert `lines` before or after the given position (anchor or
    /// `BOF`/`EOF`).
    Insert {
        reason: String,
        side: InsertSide,
        pos: AnchorOrBoundary,
        lines: Vec<String>,
    },
    /// Delete the inclusive range `[pos, end]` (defaults to single line).
    Delete {
        reason: String,
        pos: Anchor,
        end: Option<Anchor>,
    },
}

impl HashEdit {
    fn reason(&self) -> &str {
        match self {
            HashEdit::Replace { reason, .. }
            | HashEdit::Insert { reason, .. }
            | HashEdit::Delete { reason, .. } => reason,
        }
    }
}

/// A line whose anchor failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleAnchor {
    pub line: usize,
    pub expected: String,
    pub actual: String,
}

/// Error from applying hashline edits. Variants chosen so a CLI adapter
/// can render `StaleAnchors` with fresh-anchor context for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// A request-level invariant was violated (empty reason, bad range, …).
    InvalidEdit(String),
    /// At least one anchor's hash no longer matches the file.
    StaleAnchors {
        stale: Vec<StaleAnchor>,
        /// The current file contents, line-by-line, so the adapter can
        /// re-render the affected region with fresh anchors.
        file_lines: Vec<String>,
    },
    /// Two edits would touch overlapping line ranges.
    OverlappingEdits {
        first: usize,
        second: usize,
        detail: String,
    },
    /// An anchor refers to a line that doesn't exist in the file.
    OutOfRange { line: usize, total_lines: usize },
    /// Applying the edits produced the same text as the input.
    NoChange,
}

impl ApplyError {
    /// Render a model-friendly error message. For `StaleAnchors`, the
    /// affected lines are reprinted with fresh `LINEhh|TEXT` anchors and
    /// the stale lines marked with `*`.
    pub fn display(&self) -> String {
        match self {
            ApplyError::InvalidEdit(message) => message.clone(),
            ApplyError::StaleAnchors { stale, file_lines } => {
                render_stale_anchor_message(stale, file_lines)
            }
            ApplyError::OverlappingEdits {
                first,
                second,
                detail,
            } => {
                format!(
                    "edits[{first}] and edits[{second}] overlap: {detail}. Merge them or target disjoint regions."
                )
            }
            ApplyError::OutOfRange { line, total_lines } => {
                format!("Line {line} does not exist (file has {total_lines} line(s)).")
            }
            ApplyError::NoChange => {
                "No changes made. The edits produced identical content.".to_string()
            }
        }
    }
}

/// Lines of context shown on either side of a stale anchor when rendering
/// the mismatch error.
const MISMATCH_CONTEXT: usize = 2;

fn render_stale_anchor_message(stale: &[StaleAnchor], file_lines: &[String]) -> String {
    let noun = if stale.len() == 1 {
        "anchor"
    } else {
        "anchors"
    };
    let verb = if stale.len() == 1 { "does" } else { "do" };
    let mut out = format!(
        "Edit rejected: {} {noun} {verb} not match the current file (marked *). The edit was NOT applied; use the updated anchors below and retry.\n\n",
        stale.len()
    );

    let stale_lines: std::collections::HashSet<usize> = stale.iter().map(|s| s.line).collect();

    // Collect every line we need to display: each stale line ± MISMATCH_CONTEXT.
    let mut display_lines: Vec<usize> =
        std::collections::BTreeSet::from_iter(stale.iter().flat_map(|s| {
            let lo = s.line.saturating_sub(MISMATCH_CONTEXT).max(1);
            let hi = (s.line + MISMATCH_CONTEXT).min(file_lines.len());
            lo..=hi
        }))
        .into_iter()
        .collect();
    display_lines.sort_unstable();

    let mut previous: Option<usize> = None;
    for line_num in display_lines {
        if let Some(prev) = previous
            && line_num > prev + 1
        {
            out.push_str("...\n");
        }
        previous = Some(line_num);
        let content = file_lines.get(line_num - 1).map_or("", String::as_str);
        let marker = if stale_lines.contains(&line_num) {
            '*'
        } else {
            ' '
        };
        let _ = writeln!(
            out,
            "{marker}{}{}{}{}",
            line_num,
            compute_line_hash(line_num, content),
            BODY_SEP,
            content
        );
    }
    out
}

/// Result of a successful apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// New file content (lines joined with `\n`, no trailing newline added).
    pub text: String,
    /// 1-indexed line number of the first changed line in the *new* file,
    /// or `None` if nothing changed (which is itself an error before this
    /// is returned, but kept as `Option` for forward compatibility).
    pub first_changed_line: Option<usize>,
}

/// Apply a batch of hashline edits to `text`.
///
/// All anchors are validated against the original `text` before any
/// splice. On stale anchor, returns `ApplyError::StaleAnchors` and leaves
/// the caller's data untouched. Edits are applied bottom-up by end-line
/// so earlier indices stay valid through later splices.
pub fn apply_hash_edits(text: &str, edits: &[HashEdit]) -> Result<Applied, ApplyError> {
    if edits.is_empty() {
        return Err(ApplyError::InvalidEdit(
            "edits must contain at least one operation".to_string(),
        ));
    }
    for (idx, edit) in edits.iter().enumerate() {
        if edit.reason().trim().is_empty() {
            return Err(ApplyError::InvalidEdit(format!(
                "edits[{idx}].reason must not be empty"
            )));
        }
    }

    // Split into logical content lines, keeping the trailing-newline
    // info separately so the round-trip preserves file shape. A file
    // ending in '\n' has the same logical line count as one without,
    // anchored on the content; the newline is restored on output.
    let (file_lines, has_trailing_newline) = split_logical_lines(text);

    // Validate anchors first. Collect all stale anchors so the model sees
    // them in one shot instead of fixing them one at a time.
    let mut stale: Vec<StaleAnchor> = Vec::new();
    for edit in edits {
        for anchor in anchors_of(edit) {
            if anchor.line < 1 || anchor.line > file_lines.len() {
                return Err(ApplyError::OutOfRange {
                    line: anchor.line,
                    total_lines: file_lines.len(),
                });
            }
            let hash_str = std::str::from_utf8(&anchor.hash).unwrap();
            if hash_str == RANGE_INTERIOR_HASH {
                continue;
            }
            let actual = compute_line_hash(anchor.line, &file_lines[anchor.line - 1]);
            if actual != hash_str {
                stale.push(StaleAnchor {
                    line: anchor.line,
                    expected: hash_str.to_string(),
                    actual: actual.to_string(),
                });
            }
        }
    }
    if !stale.is_empty() {
        return Err(ApplyError::StaleAnchors { stale, file_lines });
    }

    // Lower every edit to a concrete [start_line, end_line] line span on
    // the *original* file plus the replacement lines. Order matters for
    // overlap detection and bottom-up application.
    let mut spans: Vec<EditSpan> = Vec::with_capacity(edits.len());
    for (idx, edit) in edits.iter().enumerate() {
        spans.push(lower_to_span(idx, edit, file_lines.len())?);
    }

    // Overlap check on original-file line spans only (inserts at the same
    // boundary count as overlap to keep ordering deterministic).
    for i in 0..spans.len() {
        for j in (i + 1)..spans.len() {
            if spans_overlap(&spans[i], &spans[j]) {
                return Err(ApplyError::OverlappingEdits {
                    first: spans[i].index,
                    second: spans[j].index,
                    detail: format!(
                        "{} and {} both touch the same region",
                        spans[i].describe(),
                        spans[j].describe()
                    ),
                });
            }
        }
    }

    // Apply bottom-up. Sort by end-line descending; for inserts at the
    // same boundary, later submitted indices win so the in-order list of
    // payloads still reads top-down in the file.
    spans.sort_by(|a, b| b.end_line.cmp(&a.end_line).then(a.index.cmp(&b.index)));

    let mut working: Vec<String> = file_lines;
    let mut first_changed: Option<usize> = None;
    for span in &spans {
        let (start_idx, end_idx_exclusive) = span.splice_range(&working);
        let pre_len = working.len();
        working.splice(
            start_idx..end_idx_exclusive,
            span.replacement.iter().cloned(),
        );
        let post_len = working.len();
        let touched_line = start_idx + 1;
        let touched_line = if span.replacement.is_empty() && post_len < pre_len {
            // pure deletion: first changed line is whatever now sits at start_idx
            touched_line.min(post_len.max(1))
        } else {
            touched_line
        };
        track_first_changed(&mut first_changed, touched_line);
    }

    let mut new_text = working.join("\n");
    if has_trailing_newline && !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    if new_text == text {
        return Err(ApplyError::NoChange);
    }
    Ok(Applied {
        text: new_text,
        first_changed_line: first_changed,
    })
}

/// Split `text` into logical content lines, returning the lines and
/// whether the input ended with a trailing newline. The trailing newline
/// is *not* represented as an empty line in the output — callers see only
/// the content lines and the boolean.
fn split_logical_lines(text: &str) -> (Vec<String>, bool) {
    if text.is_empty() {
        return (vec![String::new()], false);
    }
    let has_trailing = text.ends_with('\n');
    let trimmed = if has_trailing {
        &text[..text.len() - 1]
    } else {
        text
    };
    (
        trimmed.split('\n').map(str::to_owned).collect(),
        has_trailing,
    )
}

fn track_first_changed(slot: &mut Option<usize>, line: usize) {
    *slot = Some(match *slot {
        Some(cur) if cur <= line => cur,
        _ => line,
    });
}

/// Internal lowered representation of an edit, expressed as a span on the
/// *original* file's lines plus the replacement payload.
#[derive(Debug, Clone)]
struct EditSpan {
    index: usize,
    /// 1-indexed inclusive start line on the original file. For pure
    /// inserts at a boundary, `start_line == end_line + 1` (an empty span
    /// at the insertion point).
    start_line: usize,
    /// 1-indexed inclusive end line on the original file. For pure
    /// inserts, `end_line == start_line - 1`.
    end_line: usize,
    /// Lines to splice in (empty for deletes).
    replacement: Vec<String>,
}

impl EditSpan {
    fn describe(&self) -> String {
        if self.start_line > self.end_line {
            format!("insert at line {}", self.start_line)
        } else if self.start_line == self.end_line {
            format!("edit at line {}", self.start_line)
        } else {
            format!("edit at lines {}..={}", self.start_line, self.end_line)
        }
    }

    /// Convert the original-file 1-indexed span into a Rust 0-indexed
    /// `Range` on the working `Vec<String>` for `Vec::splice`. Pure
    /// inserts collapse to an empty range at the insertion point.
    fn splice_range(&self, working: &[String]) -> (usize, usize) {
        if self.start_line > self.end_line {
            // Pure insert at boundary `start_line` (1-indexed); 0-indexed
            // insertion point is `start_line - 1`. Clamp to `working.len()`
            // so an EOF insert against a possibly-empty file stays valid.
            let idx = (self.start_line - 1).min(working.len());
            (idx, idx)
        } else {
            (self.start_line - 1, self.end_line)
        }
    }
}

fn anchors_of(edit: &HashEdit) -> Vec<Anchor> {
    match edit {
        HashEdit::Replace { pos, end, .. } | HashEdit::Delete { pos, end, .. } => {
            let mut out = vec![*pos];
            if let Some(end) = end {
                out.push(*end);
            }
            out
        }
        HashEdit::Insert { pos, .. } => match pos {
            AnchorOrBoundary::Anchor(a) => vec![*a],
            AnchorOrBoundary::BeginningOfFile | AnchorOrBoundary::EndOfFile => vec![],
        },
    }
}

fn lower_to_span(
    index: usize,
    edit: &HashEdit,
    total_lines: usize,
) -> Result<EditSpan, ApplyError> {
    match edit {
        HashEdit::Replace {
            pos, end, lines, ..
        } => {
            let (start_line, end_line) = range_bounds(pos, end.as_ref())?;
            Ok(EditSpan {
                index,
                start_line,
                end_line,
                replacement: lines.clone(),
            })
        }
        HashEdit::Delete { pos, end, .. } => {
            let (start_line, end_line) = range_bounds(pos, end.as_ref())?;
            Ok(EditSpan {
                index,
                start_line,
                end_line,
                replacement: Vec::new(),
            })
        }
        HashEdit::Insert {
            side, pos, lines, ..
        } => {
            if lines.is_empty() {
                return Err(ApplyError::InvalidEdit(format!(
                    "edits[{index}]: insert must include at least one line"
                )));
            }
            let insertion_point: usize = match (pos, side) {
                (AnchorOrBoundary::Anchor(a), InsertSide::Before) => a.line,
                (AnchorOrBoundary::Anchor(a), InsertSide::After) => a.line + 1,
                (AnchorOrBoundary::BeginningOfFile, InsertSide::Before) => 1,
                (AnchorOrBoundary::EndOfFile, InsertSide::After) => total_lines + 1,
                (AnchorOrBoundary::BeginningOfFile, InsertSide::After) => {
                    return Err(ApplyError::InvalidEdit(format!(
                        "edits[{index}]: insert_after BOF is not allowed; use insert_before BOF"
                    )));
                }
                (AnchorOrBoundary::EndOfFile, InsertSide::Before) => {
                    return Err(ApplyError::InvalidEdit(format!(
                        "edits[{index}]: insert_before EOF is not allowed; use insert_after EOF"
                    )));
                }
            };
            Ok(EditSpan {
                index,
                start_line: insertion_point,
                end_line: insertion_point.saturating_sub(1),
                replacement: lines.clone(),
            })
        }
    }
}

fn range_bounds(pos: &Anchor, end: Option<&Anchor>) -> Result<(usize, usize), ApplyError> {
    let start = pos.line;
    let end_line = end.map_or(pos.line, |a| a.line);
    if end_line < start {
        return Err(ApplyError::InvalidEdit(format!(
            "range {}..{} ends before it starts",
            pos.display(),
            end.map_or_else(|| pos.display(), |e| e.display())
        )));
    }
    Ok((start, end_line))
}

fn spans_overlap(a: &EditSpan, b: &EditSpan) -> bool {
    // Treat pure inserts as point-like spans at their insertion point. An
    // insert overlaps another span when the insertion point falls inside
    // (start_line..=end_line). Two inserts at the exact same insertion
    // point are considered conflicting so order is unambiguous.
    let a_insert = a.start_line > a.end_line;
    let b_insert = b.start_line > b.end_line;
    match (a_insert, b_insert) {
        (true, true) => a.start_line == b.start_line,
        (true, false) => a.start_line >= b.start_line && a.start_line <= b.end_line,
        (false, true) => b.start_line >= a.start_line && b.start_line <= a.end_line,
        (false, false) => a.start_line <= b.end_line && b.start_line <= a.end_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashline::{AnchorOrBoundary, InsertSide, compute_line_hash};

    fn anchor_for(line: usize, text: &str) -> Anchor {
        let body = text.split('\n').nth(line - 1).unwrap();
        let token = format!("{line}{}", compute_line_hash(line, body));
        Anchor::parse(&token).unwrap()
    }

    #[test]
    fn replace_single_line_with_matching_anchor() {
        let original = "alpha\nbeta\ngamma\n";
        let edit = HashEdit::Replace {
            reason: "uppercase beta".into(),
            pos: anchor_for(2, original),
            end: None,
            lines: vec!["BETA".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "alpha\nBETA\ngamma\n");
        assert_eq!(applied.first_changed_line, Some(2));
    }

    #[test]
    fn replace_range_with_matching_anchors() {
        let original = "a\nb\nc\nd\n";
        let edit = HashEdit::Replace {
            reason: "swap middle".into(),
            pos: anchor_for(2, original),
            end: Some(anchor_for(3, original)),
            lines: vec!["B".into(), "C".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "a\nB\nC\nd\n");
        assert_eq!(applied.first_changed_line, Some(2));
    }

    #[test]
    fn delete_range_with_matching_anchors() {
        let original = "keep1\ndrop1\ndrop2\nkeep2\n";
        let edit = HashEdit::Delete {
            reason: "remove drops".into(),
            pos: anchor_for(2, original),
            end: Some(anchor_for(3, original)),
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "keep1\nkeep2\n");
    }

    #[test]
    fn delete_single_line_with_matching_anchor() {
        let original = "keep this\ndelete this\nkeep that too\n";
        let edit = HashEdit::Delete {
            reason: "remove middle".into(),
            pos: anchor_for(2, original),
            end: None,
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "keep this\nkeep that too\n");
        assert_eq!(applied.first_changed_line, Some(2));
    }

    #[test]
    fn insert_before_anchor() {
        let original = "a\nb\nc\n";
        let edit = HashEdit::Insert {
            reason: "header".into(),
            side: InsertSide::Before,
            pos: AnchorOrBoundary::Anchor(anchor_for(2, original)),
            lines: vec!["INSERTED".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "a\nINSERTED\nb\nc\n");
    }

    #[test]
    fn insert_after_anchor() {
        let original = "a\nb\nc\n";
        let edit = HashEdit::Insert {
            reason: "after".into(),
            side: InsertSide::After,
            pos: AnchorOrBoundary::Anchor(anchor_for(2, original)),
            lines: vec!["INSERTED".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "a\nb\nINSERTED\nc\n");
    }

    #[test]
    fn insert_before_bof_prepends() {
        let original = "a\nb\n";
        let edit = HashEdit::Insert {
            reason: "prepend".into(),
            side: InsertSide::Before,
            pos: AnchorOrBoundary::BeginningOfFile,
            lines: vec!["# header".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "# header\na\nb\n");
        assert_eq!(applied.first_changed_line, Some(1));
    }

    #[test]
    fn insert_after_eof_appends() {
        let original = "a\nb\n";
        let edit = HashEdit::Insert {
            reason: "append".into(),
            side: InsertSide::After,
            pos: AnchorOrBoundary::EndOfFile,
            lines: vec!["# footer".into()],
        };
        let applied = apply_hash_edits(original, &[edit]).unwrap();
        assert_eq!(applied.text, "a\nb\n# footer\n");
    }

    #[test]
    fn mixed_op_batch_applies_against_original_line_numbers() {
        // Mirrors demo scenario 9: a delete, a replace, and an insert_before
        // submitted in display order against the *original* file. Anchors are
        // never renumbered by the caller; the engine sorts bottom-up internally.
        let original =
            "keep header\nto-delete\nto-replace\nkeep middle\nto-prepend-above\nkeep footer\n";
        let edits = vec![
            HashEdit::Delete {
                reason: "drop line 2".into(),
                pos: anchor_for(2, original),
                end: None,
            },
            HashEdit::Replace {
                reason: "rewrite line 3".into(),
                pos: anchor_for(3, original),
                end: None,
                lines: vec!["replaced line 3".into()],
            },
            HashEdit::Insert {
                reason: "insert above line 5".into(),
                side: InsertSide::Before,
                pos: AnchorOrBoundary::Anchor(anchor_for(5, original)),
                lines: vec!["inserted before 5".into()],
            },
        ];
        let applied = apply_hash_edits(original, &edits).unwrap();
        assert_eq!(
            applied.text,
            "keep header\nreplaced line 3\nkeep middle\ninserted before 5\nto-prepend-above\nkeep footer\n"
        );
        // First changed line in the post-edit file is line 2 (was "to-delete",
        // now "replaced line 3").
        assert_eq!(applied.first_changed_line, Some(2));
    }

    #[test]
    fn multiple_edits_apply_against_original_line_numbers() {
        let original = "a\nb\nc\nd\n";
        let edits = vec![
            HashEdit::Replace {
                reason: "upper a".into(),
                pos: anchor_for(1, original),
                end: None,
                lines: vec!["A".into()],
            },
            HashEdit::Replace {
                reason: "upper d".into(),
                pos: anchor_for(4, original),
                end: None,
                lines: vec!["D".into()],
            },
        ];
        let applied = apply_hash_edits(original, &edits).unwrap();
        assert_eq!(applied.text, "A\nb\nc\nD\n");
        assert_eq!(applied.first_changed_line, Some(1));
    }

    #[test]
    fn stale_anchor_returns_structured_error_and_leaves_text_untouched() {
        let original = "alpha\nbeta\ngamma\n";
        let mut wrong = anchor_for(2, original);
        // Flip the hash so it definitely doesn't match.
        wrong.hash = [b'z', b'z'];
        let edit = HashEdit::Replace {
            reason: "stale".into(),
            pos: wrong,
            end: None,
            lines: vec!["BETA".into()],
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        match err {
            ApplyError::StaleAnchors { stale, file_lines } => {
                assert_eq!(stale.len(), 1);
                assert_eq!(stale[0].line, 2);
                assert_eq!(stale[0].expected, "zz");
                assert_eq!(file_lines[1], "beta");
            }
            other => panic!("expected StaleAnchors, got {other:?}"),
        }
    }

    #[test]
    fn stale_anchor_display_contains_fresh_anchors_and_star_markers() {
        let original = "one\ntwo\nthree\nfour\nfive\n";
        let mut wrong = anchor_for(3, original);
        wrong.hash = [b'z', b'z'];
        let edit = HashEdit::Replace {
            reason: "stale".into(),
            pos: wrong,
            end: None,
            lines: vec!["THREE".into()],
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        let message = err.display();
        // The actual hash for line 3 must appear; the expected `zz` must not.
        let actual = compute_line_hash(3, "three");
        assert!(message.contains(&format!("*3{actual}|three")), "{message}");
        // Context lines 1, 2, 4, 5 should be present without `*`.
        assert!(message.contains("|two"));
        assert!(message.contains("|four"));
    }

    #[test]
    fn out_of_range_line_returns_out_of_range_error() {
        let original = "only line\n";
        let edit = HashEdit::Delete {
            reason: "del".into(),
            pos: Anchor {
                line: 99,
                hash: *b"aa",
            },
            end: None,
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        matches!(err, ApplyError::OutOfRange { line: 99, .. });
    }

    #[test]
    fn overlapping_edits_are_rejected() {
        let original = "a\nb\nc\nd\n";
        let edits = vec![
            HashEdit::Replace {
                reason: "first".into(),
                pos: anchor_for(2, original),
                end: Some(anchor_for(3, original)),
                lines: vec!["X".into()],
            },
            HashEdit::Replace {
                reason: "second".into(),
                pos: anchor_for(3, original),
                end: Some(anchor_for(4, original)),
                lines: vec!["Y".into()],
            },
        ];
        let err = apply_hash_edits(original, &edits).unwrap_err();
        assert!(matches!(err, ApplyError::OverlappingEdits { .. }));
    }

    #[test]
    fn empty_edits_list_is_rejected() {
        let err = apply_hash_edits("hello\n", &[]).unwrap_err();
        assert!(matches!(err, ApplyError::InvalidEdit(_)));
    }

    #[test]
    fn empty_reason_is_rejected() {
        let original = "a\n";
        let edit = HashEdit::Replace {
            reason: " ".into(),
            pos: anchor_for(1, original),
            end: None,
            lines: vec!["A".into()],
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        assert!(matches!(err, ApplyError::InvalidEdit(_)));
    }

    #[test]
    fn no_change_is_rejected() {
        let original = "alpha\n";
        let edit = HashEdit::Replace {
            reason: "noop".into(),
            pos: anchor_for(1, original),
            end: None,
            lines: vec!["alpha".into()],
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        assert!(matches!(err, ApplyError::NoChange));
    }

    #[test]
    fn range_with_end_before_start_is_rejected() {
        let original = "a\nb\nc\n";
        let edit = HashEdit::Replace {
            reason: "bad range".into(),
            pos: anchor_for(3, original),
            end: Some(anchor_for(2, original)),
            lines: vec!["X".into()],
        };
        let err = apply_hash_edits(original, &[edit]).unwrap_err();
        assert!(matches!(err, ApplyError::InvalidEdit(_)));
    }
}
