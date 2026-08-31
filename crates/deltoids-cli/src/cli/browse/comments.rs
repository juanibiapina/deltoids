//! Session-only review comments, shared by both TUI modes.
//!
//! A user browsing any diff — the working tree in Files mode or a trace
//! entry in Traces mode — can attach a free-text note to a diff line and
//! then copy every note as one agent-ready prompt. Comments live in
//! memory for the running session; nothing is written to disk.
//!
//! This module is the pure core. A comment is keyed by [`CommentAnchor`]:
//! the diff it belongs to ([`CommentScope`]), the file path, which side of
//! the diff the line is on, and its file line number. Deliberately *not*
//! keyed by hunk/line indices: Files mode re-diffs a moving working tree,
//! where indices shift on every keystroke elsewhere in the repo, while
//! line numbers do not.
//!
//! Copying never re-reads the diff: [`Comment`] keeps the line's text and
//! kind as they were when the note was written, so a prompt is always
//! internally consistent. The current diff is used only to order the
//! items the way the user sees them ([`build_prompt`]).

use std::collections::HashMap;

use deltoids::{Hunk, LineKind};

/// Which diff a comment belongs to.
///
/// Files mode has exactly one (the working tree); Traces mode has one per
/// trace entry, so notes on the same path in different edits coexist.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CommentScope {
    WorkingTree,
    TraceEntry {
        trace_id: String,
        entry_index: usize,
    },
}

/// Which side of the diff a line belongs to: removed lines are numbered
/// against the old file, added and context lines against the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LineSide {
    Old,
    New,
}

/// What a comment is attached to. Unique per logical diff line: hunks
/// never overlap, and inside a hunk each side's counter advances at most
/// once per line, so `(side, line)` names exactly one line of one file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CommentAnchor {
    pub(super) scope: CommentScope,
    pub(super) path: String,
    pub(super) side: LineSide,
    pub(super) line: usize,
}

/// A note plus a snapshot of the line it was written against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Comment {
    pub(super) note: String,
    /// The line's text when the note was written; what the prompt shows.
    pub(super) code: String,
    pub(super) kind: LineKind,
}

/// In-memory map of anchors to comments.
#[derive(Debug, Default, Clone)]
pub(super) struct CommentStore {
    comments: HashMap<CommentAnchor, Comment>,
}

impl CommentStore {
    pub(super) fn get(&self, anchor: &CommentAnchor) -> Option<&Comment> {
        self.comments.get(anchor)
    }

    /// The note text at `anchor`, if any.
    pub(super) fn note(&self, anchor: &CommentAnchor) -> Option<&str> {
        self.get(anchor).map(|comment| comment.note.as_str())
    }

    /// Write the note at `anchor`, recording the line it annotates.
    /// Whitespace-only text removes the comment, so saving an emptied
    /// editor deletes it.
    pub(super) fn set(
        &mut self,
        anchor: CommentAnchor,
        note: String,
        code: String,
        kind: LineKind,
    ) {
        if note.trim().is_empty() {
            self.remove(&anchor);
        } else {
            self.comments.insert(anchor, Comment { note, code, kind });
        }
    }

    pub(super) fn remove(&mut self, anchor: &CommentAnchor) {
        self.comments.remove(anchor);
    }

    /// Drop every comment, returning how many were removed. Used by the
    /// `D` binding after a reviewer has copied notes and wants to hand
    /// off cleanly without re-emitting them.
    pub(super) fn clear(&mut self) -> usize {
        let count = self.comments.len();
        self.comments.clear();
        count
    }

    pub(super) fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }
}

/// One diff on screen, in display order: everything [`build_prompt`]
/// needs to order the comments anchored in it.
pub(super) struct PromptSection<'a> {
    pub(super) scope: CommentScope,
    pub(super) path: String,
    pub(super) hunks: &'a [Hunk],
}

const PROMPT_INSTRUCTION: &str = "Address the following code review comments. For each, the file and line\nare given, with the relevant line and the reviewer's note.\nA line marked outdated is quoted as it read when the note was written; it has changed since, so locate the reviewer's intent rather than trusting the number.";

/// Build the review prompt for every comment in `store`, ordered the way
/// the user sees them: `sections` in display order, and within a section
/// in diff order. Comments whose anchor no longer appears in any section
/// (the line was committed away, reverted, or edited out) are kept, in
/// path/line order, after the anchored ones — a note is never dropped
/// silently.
///
/// `cwd` strips a leading working-directory prefix from absolute paths;
/// pass `""` when paths are already relative. Returns the prompt and its
/// comment count, or `None` when there is nothing to copy.
pub(super) fn build_prompt(
    cwd: &str,
    store: &CommentStore,
    sections: &[PromptSection<'_>],
) -> Option<(String, usize)> {
    if store.is_empty() {
        return None;
    }

    let mut ordered: Vec<(&CommentAnchor, &Comment)> = Vec::new();
    for section in sections {
        let anchors = section.hunks.iter().flat_map(|hunk| {
            hunk_lines(hunk).map(|line| CommentAnchor {
                scope: section.scope.clone(),
                path: section.path.clone(),
                side: line.side,
                line: line.number,
            })
        });
        for (anchor, comment) in anchors.filter_map(|anchor| store.comments.get_key_value(&anchor))
        {
            // Hunks expanded to their enclosing scope can overlap, so the
            // same line may be walked more than once. One line, one item.
            if !ordered.iter().any(|(seen, _)| *seen == anchor) {
                ordered.push((anchor, comment));
            }
        }
    }

    let mut orphans: Vec<(&CommentAnchor, &Comment)> = store
        .comments
        .iter()
        .filter(|(anchor, _)| !ordered.iter().any(|(seen, _)| *seen == *anchor))
        .collect();
    orphans.sort_by(|(a, _), (b, _)| {
        (&a.path, a.line, a.side == LineSide::New).cmp(&(&b.path, b.line, b.side == LineSide::New))
    });
    ordered.extend(orphans);

    if ordered.is_empty() {
        return None;
    }
    let count = ordered.len();

    let mut out = String::from(PROMPT_INSTRUCTION);
    for (anchor, comment) in ordered {
        let path = relativize(cwd, &anchor.path);
        let marker = line_marker(&comment.kind);
        let outdated = !comment_is_current(anchor, comment, sections);
        let outdated_marker = if outdated { " (outdated)" } else { "" };
        out.push_str("\n\n");
        out.push_str(&format!("{path}:{}{outdated_marker}\n", anchor.line));
        out.push_str(&format!("{marker} {}\n", comment.code));
        out.push_str(&format!("note: {}", comment.note));
    }
    out.push('\n');
    Some((out, count))
}

fn comment_is_current(
    anchor: &CommentAnchor,
    comment: &Comment,
    sections: &[PromptSection<'_>],
) -> bool {
    sections.iter().any(|section| {
        section.scope == anchor.scope
            && section.path == anchor.path
            && section.hunks.iter().flat_map(hunk_lines).any(|line| {
                line.side == anchor.side
                    && line.number == anchor.line
                    && line.content == comment.code
            })
    })
}

/// Follow comments onto the lines they were written against after the
/// underlying diff changed.
///
/// A working tree moves under the reviewer: editing anything above a
/// commented line shifts its number, which would otherwise leave the note
/// pointing at whatever now occupies that number. For every comment whose
/// anchored line no longer carries the text it was written against, this
/// looks for exactly one line on the same side of the same file with that
/// text and moves the anchor there. Ambiguous or vanished lines are left
/// alone; the diff pane marks them outdated.
pub(super) fn reanchor(store: &mut CommentStore, sections: &[PromptSection<'_>]) {
    let mut moves: Vec<(CommentAnchor, CommentAnchor)> = Vec::new();

    for section in sections {
        let lines: Vec<HunkLine<'_>> = section.hunks.iter().flat_map(hunk_lines).collect();

        let mine = store
            .comments
            .iter()
            .filter(|(anchor, _)| anchor.scope == section.scope && anchor.path == section.path);
        for (anchor, comment) in mine {
            let anchored = lines
                .iter()
                .find(|line| line.side == anchor.side && line.number == anchor.line);
            if anchored.is_some_and(|line| line.content == comment.code) {
                continue;
            }
            // Overlapping hunks can render one line twice, so count
            // distinct line numbers rather than occurrences.
            let mut candidates: Vec<usize> = lines
                .iter()
                .filter(|line| line.side == anchor.side && line.content == comment.code)
                .map(|line| line.number)
                .collect();
            candidates.sort_unstable();
            candidates.dedup();
            let [found] = candidates[..] else {
                continue;
            };
            moves.push((
                anchor.clone(),
                CommentAnchor {
                    line: found,
                    ..anchor.clone()
                },
            ));
        }
    }

    for (from, to) in moves {
        if let Some(comment) = store.comments.remove(&from) {
            store.comments.insert(to, comment);
        }
    }
}

/// Diff-side marker shown before the code line in the prompt.
fn line_marker(kind: &LineKind) -> char {
    match kind {
        LineKind::Added => '+',
        LineKind::Removed => '-',
        LineKind::Context => ' ',
    }
}

/// Strip a leading `cwd/` from `path` so prompts show repo-relative
/// paths. A no-op for an empty `cwd` or a path outside it.
fn relativize(cwd: &str, path: &str) -> String {
    if cwd.is_empty() {
        return path.to_string();
    }
    let prefix = if cwd.ends_with('/') {
        cwd.to_string()
    } else {
        format!("{cwd}/")
    };
    path.strip_prefix(&prefix).unwrap_or(path).to_string()
}

/// One logical line of a hunk, resolved to what a comment needs: where it
/// sits in the file and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HunkLine<'a> {
    /// Index into [`Hunk::lines`], matching `HunkRow::source_line`.
    pub(super) index: usize,
    pub(super) side: LineSide,
    /// File line number: new-file numbering for added and context lines,
    /// old-file numbering for removed lines.
    pub(super) number: usize,
    pub(super) kind: &'a LineKind,
    pub(super) content: &'a str,
}

/// Walk a hunk's logical lines, numbering each against the side it
/// belongs to. The one place diff-line numbering lives; both the row
/// builders and the prompt use it.
pub(super) fn hunk_lines(hunk: &Hunk) -> impl Iterator<Item = HunkLine<'_>> {
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    hunk.lines.iter().enumerate().map(move |(index, line)| {
        let (side, number) = match line.kind {
            LineKind::Removed => (LineSide::Old, old_line),
            _ => (LineSide::New, new_line),
        };
        match line.kind {
            LineKind::Context => {
                old_line += 1;
                new_line += 1;
            }
            LineKind::Added => new_line += 1,
            LineKind::Removed => old_line += 1,
        }
        HunkLine {
            index,
            side,
            number,
            kind: &line.kind,
            content: &line.content,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::DiffLine;

    fn diff_line(kind: LineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
        }
    }

    /// A hunk starting at old/new line 10: one context line, one removed,
    /// one added.
    fn sample_hunk() -> Hunk {
        Hunk {
            old_start: 10,
            new_start: 10,
            lines: vec![
                diff_line(LineKind::Context, "fn main() {"),
                diff_line(LineKind::Removed, "let x = 1;"),
                diff_line(LineKind::Added, "let x = 2;"),
            ],
            ancestors: Vec::new(),
        }
    }

    fn anchor(path: &str, side: LineSide, line: usize) -> CommentAnchor {
        CommentAnchor {
            scope: CommentScope::WorkingTree,
            path: path.to_string(),
            side,
            line,
        }
    }

    fn note_at(store: &mut CommentStore, anchor: CommentAnchor, note: &str, code: &str) {
        store.set(anchor, note.to_string(), code.to_string(), LineKind::Added);
    }

    fn section<'a>(path: &str, hunks: &'a [Hunk]) -> PromptSection<'a> {
        PromptSection {
            scope: CommentScope::WorkingTree,
            path: path.to_string(),
            hunks,
        }
    }

    #[test]
    fn store_round_trips_a_comment() {
        let mut store = CommentStore::default();
        let a = anchor("a.rs", LineSide::New, 10);
        assert_eq!(store.note(&a), None);

        note_at(&mut store, a.clone(), "hello", "let x = 2;");
        assert_eq!(store.note(&a), Some("hello"));
        assert_eq!(store.get(&a).map(|c| c.code.as_str()), Some("let x = 2;"));

        note_at(&mut store, a.clone(), "edited", "let x = 2;");
        assert_eq!(store.note(&a), Some("edited"));

        store.remove(&a);
        assert_eq!(store.note(&a), None);
        assert!(store.is_empty());
    }

    #[test]
    fn clear_empties_the_store_and_returns_the_count() {
        let mut store = CommentStore::default();
        note_at(&mut store, anchor("a.rs", LineSide::New, 10), "one", "c");
        note_at(&mut store, anchor("b.rs", LineSide::New, 20), "two", "c");
        assert_eq!(store.clear(), 2);
        assert!(store.is_empty());
        // Clearing an empty store removes nothing.
        assert_eq!(store.clear(), 0);
    }

    #[test]
    fn whitespace_only_text_removes_the_comment() {
        let mut store = CommentStore::default();
        let a = anchor("a.rs", LineSide::New, 10);
        note_at(&mut store, a.clone(), "note", "code");
        note_at(&mut store, a.clone(), "   ", "code");
        assert_eq!(store.note(&a), None);
    }

    #[test]
    fn the_same_line_in_different_scopes_holds_different_comments() {
        let mut store = CommentStore::default();
        let working = anchor("a.rs", LineSide::New, 10);
        let mut traced = working.clone();
        traced.scope = CommentScope::TraceEntry {
            trace_id: "T1".to_string(),
            entry_index: 0,
        };
        note_at(&mut store, working.clone(), "in the tree", "code");
        note_at(&mut store, traced.clone(), "in the trace", "code");

        assert_eq!(store.note(&working), Some("in the tree"));
        assert_eq!(store.note(&traced), Some("in the trace"));
    }

    #[test]
    fn hunk_lines_number_each_side_independently() {
        let hunk = sample_hunk();
        let lines: Vec<HunkLine<'_>> = hunk_lines(&hunk).collect();

        assert_eq!(lines[0].side, LineSide::New);
        assert_eq!(lines[0].number, 10);
        // The removed line is old-file line 11 and does not advance the
        // new-file counter, so the added line that replaces it is new 11.
        assert_eq!(lines[1].side, LineSide::Old);
        assert_eq!(lines[1].number, 11);
        assert_eq!(lines[2].side, LineSide::New);
        assert_eq!(lines[2].number, 11);
        // Indices match `HunkRow::source_line`.
        assert_eq!(
            lines.iter().map(|l| l.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn hunk_lines_keep_numbering_across_hunk_starts() {
        let hunk = Hunk {
            old_start: 100,
            new_start: 200,
            lines: vec![
                diff_line(LineKind::Removed, "gone"),
                diff_line(LineKind::Context, "kept"),
            ],
            ancestors: Vec::new(),
        };
        let lines: Vec<HunkLine<'_>> = hunk_lines(&hunk).collect();
        assert_eq!((lines[0].side, lines[0].number), (LineSide::Old, 100));
        assert_eq!((lines[1].side, lines[1].number), (LineSide::New, 200));
    }

    #[test]
    fn prompt_carries_path_line_marker_and_note() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("src/app.rs", LineSide::New, 11),
            "handle the error".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );

        let (prompt, count) = build_prompt("", &store, &[section("src/app.rs", &[hunk])]).unwrap();
        assert_eq!(count, 1);
        assert!(prompt.starts_with("Address the following code review comments."));
        assert!(prompt.contains("src/app.rs:11\n+ let x = 2;\nnote: handle the error"));
    }

    #[test]
    fn prompt_marks_removed_lines_with_old_numbering() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::Old, 11),
            "why remove".to_string(),
            "let x = 1;".to_string(),
            LineKind::Removed,
        );
        let (prompt, _) = build_prompt("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert!(prompt.contains("a.rs:11\n- let x = 1;\nnote: why remove"));
    }

    #[test]
    fn prompt_marks_context_lines_without_a_diff_sign() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 10),
            "explain".to_string(),
            "fn main() {".to_string(),
            LineKind::Context,
        );
        let (prompt, _) = build_prompt("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert!(prompt.contains("a.rs:10\n  fn main() {\nnote: explain"));
    }

    #[test]
    fn prompt_follows_section_order_then_diff_order() {
        let first = sample_hunk();
        let second = sample_hunk();
        let mut store = CommentStore::default();
        note_at(&mut store, anchor("b.rs", LineSide::New, 10), "third", "c");
        note_at(&mut store, anchor("a.rs", LineSide::Old, 11), "second", "c");
        note_at(&mut store, anchor("a.rs", LineSide::New, 10), "first", "c");

        let (prompt, count) = build_prompt(
            "",
            &store,
            &[section("a.rs", &[first]), section("b.rs", &[second])],
        )
        .unwrap();

        assert_eq!(count, 3);
        let at = |needle: &str| prompt.find(needle).unwrap();
        assert!(at("note: first") < at("note: second"));
        assert!(at("note: second") < at("note: third"));
    }

    #[test]
    fn prompt_lists_a_line_once_even_when_hunks_overlap() {
        // Hunks expanded to their enclosing scope can cover the same line
        // twice; the reviewer wrote one note and expects one prompt item.
        let first = sample_hunk();
        let second = sample_hunk();
        let mut store = CommentStore::default();
        note_at(&mut store, anchor("a.rs", LineSide::New, 10), "once", "c");

        let (prompt, count) =
            build_prompt("", &store, &[section("a.rs", &[first, second])]).expect("one comment");
        assert_eq!(count, 1);
        assert_eq!(prompt.matches("note: once").count(), 1);
    }

    #[test]
    fn prompt_marks_a_comment_outdated_when_the_line_changed_in_place() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("a.rs", LineSide::New, 10),
            "keep the intent",
            "fn old_name() {",
        );

        let (prompt, _) = build_prompt("", &store, &[section("a.rs", &[hunk])]).unwrap();

        assert!(prompt.contains("a.rs:10 (outdated)\n+ fn old_name() {"));
        assert!(prompt.contains(
            "A line marked outdated is quoted as it read when the note was written; it has changed since, so locate the reviewer's intent rather than trusting the number."
        ));
    }

    #[test]
    fn prompt_keeps_comments_whose_line_left_the_diff() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("a.rs", LineSide::New, 10),
            "anchored",
            "c",
        );
        // A line that is no longer in any section: the user reverted or
        // committed it. The note survives, after the anchored ones.
        note_at(&mut store, anchor("z.rs", LineSide::New, 99), "orphan", "c");

        let (prompt, count) = build_prompt("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert_eq!(count, 2);
        assert!(prompt.find("note: anchored").unwrap() < prompt.find("note: orphan").unwrap());
        assert!(prompt.contains("z.rs:99 (outdated)"));
    }

    #[test]
    fn reanchor_follows_a_line_that_moved() {
        // The reviewer commented on new line 11; an edit above pushed the
        // same text down to line 21.
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 11),
            "note".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );

        let moved = Hunk {
            old_start: 20,
            new_start: 20,
            lines: vec![
                diff_line(LineKind::Context, "fn main() {"),
                diff_line(LineKind::Added, "let x = 2;"),
            ],
            ancestors: Vec::new(),
        };
        reanchor(&mut store, &[section("a.rs", &[moved])]);

        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 11)), None);
        assert_eq!(
            store.note(&anchor("a.rs", LineSide::New, 21)),
            Some("note"),
            "the comment follows its line"
        );
    }

    #[test]
    fn reanchor_leaves_a_line_that_did_not_move() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 11),
            "note".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );
        reanchor(&mut store, &[section("a.rs", &[hunk])]);
        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 11)), Some("note"));
    }

    #[test]
    fn reanchor_leaves_ambiguous_and_vanished_lines_alone() {
        // Two lines now carry the commented text: moving the note would be
        // a guess, so it stays put (and renders outdated).
        let ambiguous = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                diff_line(LineKind::Added, "let x = 2;"),
                diff_line(LineKind::Added, "let x = 2;"),
            ],
            ancestors: Vec::new(),
        };
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 11),
            "note".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );
        reanchor(&mut store, &[section("a.rs", &[ambiguous])]);
        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 11)), Some("note"));

        // The text is gone entirely: the note is kept where it was.
        let gone = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![diff_line(LineKind::Added, "something else")],
            ancestors: Vec::new(),
        };
        reanchor(&mut store, &[section("a.rs", &[gone])]);
        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 11)), Some("note"));
    }

    #[test]
    fn reanchor_follows_a_moved_line_that_overlapping_hunks_render_twice() {
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 11),
            "note".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );
        let moved = Hunk {
            old_start: 20,
            new_start: 20,
            lines: vec![
                diff_line(LineKind::Context, "fn main() {"),
                diff_line(LineKind::Added, "let x = 2;"),
            ],
            ancestors: Vec::new(),
        };
        // The same hunk twice: one line, rendered in two overlapping
        // windows. That is not an ambiguous match.
        reanchor(&mut store, &[section("a.rs", &[moved.clone(), moved])]);
        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 21)), Some("note"));
    }

    #[test]
    fn reanchor_only_touches_the_file_and_side_it_was_written_on() {
        let mut store = CommentStore::default();
        // A note on b.rs must not be moved by a.rs's diff.
        store.set(
            anchor("b.rs", LineSide::New, 11),
            "note".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );
        let moved = Hunk {
            old_start: 20,
            new_start: 20,
            lines: vec![diff_line(LineKind::Added, "let x = 2;")],
            ancestors: Vec::new(),
        };
        reanchor(&mut store, &[section("a.rs", &[moved])]);
        assert_eq!(store.note(&anchor("b.rs", LineSide::New, 11)), Some("note"));
    }

    #[test]
    fn prompt_is_none_without_comments() {
        let hunk = sample_hunk();
        let store = CommentStore::default();
        assert!(build_prompt("", &store, &[section("a.rs", &[hunk])]).is_none());
    }

    #[test]
    fn prompt_relativizes_absolute_paths_against_the_working_directory() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("/repo/src/a.rs", LineSide::New, 10),
            "note",
            "c",
        );
        let (prompt, _) = build_prompt("/repo", &store, &[section("/repo/src/a.rs", &[hunk])])
            .expect("one comment");
        assert!(prompt.contains("src/a.rs:10"));
    }

    #[test]
    fn relativize_leaves_paths_outside_the_working_directory_alone() {
        assert_eq!(relativize("/repo", "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relativize("/repo/", "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relativize("/repo", "/other/a.rs"), "/other/a.rs");
        assert_eq!(relativize("", "src/a.rs"), "src/a.rs");
    }
}
