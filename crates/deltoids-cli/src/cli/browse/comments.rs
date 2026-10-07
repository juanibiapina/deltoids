//! Session-only review comments on diff lines, shared by both TUI modes.
//!
//! Comments are keyed by file line number rather than hunk index, because
//! Files mode re-diffs the working tree and hunk indices shift on every
//! edit. Each [`Comment`] keeps the line as it read when the note was
//! written; [`review_text`] quotes that snapshot.

use std::collections::HashMap;

use deltoids::{Hunk, LineKind};

/// Which diff a comment belongs to: the working tree, or one trace entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CommentScope {
    WorkingTree,
    TraceEntry {
        trace_id: String,
        entry_index: usize,
    },
}

/// The file version a line number counts against. Staged and unstaged
/// diffs number lines against the git `Index`, between HEAD (`Old`) and
/// the worktree (`New`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LineSide {
    Old,
    Index,
    New,
}

/// The file version each side of a diff numbers its lines against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Numbering {
    pub(super) old: LineSide,
    pub(super) new: LineSide,
}

impl Numbering {
    /// HEAD → worktree, or a trace entry.
    pub(super) const PLAIN: Self = Self {
        old: LineSide::Old,
        new: LineSide::New,
    };
    /// HEAD → index.
    pub(super) const STAGED: Self = Self {
        old: LineSide::Old,
        new: LineSide::Index,
    };
    /// Index → worktree.
    pub(super) const UNSTAGED: Self = Self {
        old: LineSide::Index,
        new: LineSide::New,
    };
}

/// What a comment is attached to: one line of one file in one diff.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CommentAnchor {
    pub(super) scope: CommentScope,
    pub(super) path: String,
    pub(super) side: LineSide,
    pub(super) line: usize,
}

/// A note plus the line it was written against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Comment {
    pub(super) note: String,
    pub(super) code: String,
    pub(super) kind: LineKind,
}

#[derive(Debug, Default, Clone)]
pub(super) struct CommentStore {
    comments: HashMap<CommentAnchor, Comment>,
    revision: u64,
}

impl CommentStore {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn get(&self, anchor: &CommentAnchor) -> Option<&Comment> {
        self.comments.get(anchor)
    }

    pub(super) fn note(&self, anchor: &CommentAnchor) -> Option<&str> {
        self.get(anchor).map(|comment| comment.note.as_str())
    }

    /// Whitespace-only text removes the comment.
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
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(super) fn remove(&mut self, anchor: &CommentAnchor) {
        if self.comments.remove(anchor).is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Drop every comment, returning how many were removed.
    pub(super) fn clear(&mut self) -> usize {
        let count = self.comments.len();
        self.comments.clear();
        if count > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        count
    }

    pub(super) fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }
}

/// One diff on screen.
pub(super) struct DiffSection<'a> {
    pub(super) scope: CommentScope,
    pub(super) path: String,
    pub(super) hunks: &'a [Hunk],
    pub(super) numbering: Numbering,
}

impl DiffSection<'_> {
    fn lines(&self) -> impl Iterator<Item = HunkLine<'_>> {
        self.hunks
            .iter()
            .flat_map(|hunk| numbered_lines(hunk, self.numbering))
    }
}

/// Every comment in `store` as blocks of `path:line`, the quoted line,
/// and the note, with the text and the comment count.
///
/// Comments follow `sections` order, then diff order. Comments whose line
/// left every section come last, sorted by path and line. Paths under
/// `cwd` are made relative.
pub(super) fn review_text(
    cwd: &str,
    store: &CommentStore,
    sections: &[DiffSection<'_>],
) -> Option<(String, usize)> {
    if store.is_empty() {
        return None;
    }

    let mut ordered: Vec<(&CommentAnchor, &Comment)> = Vec::new();
    for section in sections {
        let anchors = section.lines().map(|line| CommentAnchor {
            scope: section.scope.clone(),
            path: section.path.clone(),
            side: line.side,
            line: line.number,
        });
        for (anchor, comment) in anchors.filter_map(|anchor| store.comments.get_key_value(&anchor))
        {
            // Overlapping hunks can show the same line twice.
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

    let blocks: Vec<String> = ordered
        .into_iter()
        .map(|(anchor, comment)| {
            let path = relativize(cwd, &anchor.path);
            let marker = line_marker(&comment.kind);
            let outdated = if comment_is_current(anchor, comment, sections) {
                ""
            } else {
                " (outdated)"
            };
            format!(
                "{path}:{line}{outdated}\n{marker} {code}\n{note}\n",
                line = anchor.line,
                code = comment.code,
                note = comment.note,
            )
        })
        .collect();
    Some((blocks.join("\n"), count))
}

fn comment_is_current(
    anchor: &CommentAnchor,
    comment: &Comment,
    sections: &[DiffSection<'_>],
) -> bool {
    sections.iter().any(|section| {
        section.scope == anchor.scope
            && section.path == anchor.path
            && section.lines().any(|line| {
                line.side == anchor.side
                    && line.number == anchor.line
                    && line.content == comment.code
            })
    })
}

/// Move each comment whose line number shifted onto the one line, on the
/// same side, that still has its text. An `Index` comment may move to any
/// side, since staging or unstaging a file turns index lines into HEAD or
/// worktree lines. Ambiguous or vanished lines stay put.
pub(super) fn reanchor(store: &mut CommentStore, sections: &[DiffSection<'_>]) {
    let mut moves: Vec<(CommentAnchor, CommentAnchor)> = Vec::new();

    for (anchor, comment) in &store.comments {
        let lines: Vec<HunkLine<'_>> = sections
            .iter()
            .filter(|section| section.scope == anchor.scope && section.path == anchor.path)
            .flat_map(DiffSection::lines)
            .collect();
        if lines.is_empty()
            || lines.iter().any(|line| {
                line.side == anchor.side
                    && line.number == anchor.line
                    && line.content == comment.code
            })
        {
            continue;
        }
        let matches = |any_side: bool| {
            // Overlapping hunks can show the same line twice.
            let mut found: Vec<(LineSide, usize)> = lines
                .iter()
                .filter(|line| {
                    (any_side || line.side == anchor.side) && line.content == comment.code
                })
                .map(|line| (line.side, line.number))
                .collect();
            found.sort_unstable_by_key(|(side, number)| (*side as u8, *number));
            found.dedup();
            found
        };
        let mut candidates = matches(false);
        if candidates.is_empty() && anchor.side == LineSide::Index {
            candidates = matches(true);
        }
        let [(side, line)] = candidates[..] else {
            continue;
        };
        moves.push((
            anchor.clone(),
            CommentAnchor {
                side,
                line,
                ..anchor.clone()
            },
        ));
    }

    for (from, to) in moves {
        if let Some(comment) = store.comments.remove(&from) {
            store.comments.insert(to, comment);
            store.revision = store.revision.wrapping_add(1);
        }
    }
}

fn line_marker(kind: &LineKind) -> char {
    match kind {
        LineKind::Added => '+',
        LineKind::Removed => '-',
        LineKind::Context => ' ',
    }
}

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

/// One line of a hunk with its file position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HunkLine<'a> {
    /// Index into [`Hunk::lines`], matching `HunkRow::source_line`.
    pub(super) index: usize,
    pub(super) side: LineSide,
    pub(super) number: usize,
    pub(super) kind: &'a LineKind,
    pub(super) content: &'a str,
}

pub(super) fn hunk_lines(hunk: &Hunk) -> impl Iterator<Item = HunkLine<'_>> {
    numbered_lines(hunk, Numbering::PLAIN)
}

pub(super) fn numbered_lines(
    hunk: &Hunk,
    numbering: Numbering,
) -> impl Iterator<Item = HunkLine<'_>> {
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    hunk.lines.iter().enumerate().map(move |(index, line)| {
        let (side, number) = match line.kind {
            LineKind::Removed => (numbering.old, old_line),
            _ => (numbering.new, new_line),
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

    fn section<'a>(path: &str, hunks: &'a [Hunk]) -> DiffSection<'a> {
        DiffSection {
            scope: CommentScope::WorkingTree,
            path: path.to_string(),
            hunks,
            numbering: Numbering::PLAIN,
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
    fn review_text_carries_path_line_marker_and_note() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("src/app.rs", LineSide::New, 11),
            "handle the error".to_string(),
            "let x = 2;".to_string(),
            LineKind::Added,
        );

        let (text, count) = review_text("", &store, &[section("src/app.rs", &[hunk])]).unwrap();
        assert_eq!(count, 1);
        assert_eq!(text, "src/app.rs:11\n+ let x = 2;\nhandle the error\n");
    }

    #[test]
    fn review_text_marks_removed_lines_with_old_numbering() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::Old, 11),
            "why remove".to_string(),
            "let x = 1;".to_string(),
            LineKind::Removed,
        );
        let (text, _) = review_text("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert!(text.contains("a.rs:11\n- let x = 1;\nwhy remove\n"));
    }

    #[test]
    fn review_text_marks_context_lines_without_a_diff_sign() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        store.set(
            anchor("a.rs", LineSide::New, 10),
            "explain".to_string(),
            "fn main() {".to_string(),
            LineKind::Context,
        );
        let (text, _) = review_text("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert!(text.contains("a.rs:10\n  fn main() {\nexplain\n"));
    }

    #[test]
    fn review_text_follows_section_order_then_diff_order() {
        let first = sample_hunk();
        let second = sample_hunk();
        let mut store = CommentStore::default();
        note_at(&mut store, anchor("b.rs", LineSide::New, 10), "third", "c");
        note_at(&mut store, anchor("a.rs", LineSide::Old, 11), "second", "c");
        note_at(&mut store, anchor("a.rs", LineSide::New, 10), "first", "c");

        let (text, count) = review_text(
            "",
            &store,
            &[section("a.rs", &[first]), section("b.rs", &[second])],
        )
        .unwrap();

        assert_eq!(count, 3);
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(at("\nfirst\n") < at("\nsecond\n"));
        assert!(at("\nsecond\n") < at("\nthird\n"));
    }

    #[test]
    fn review_text_lists_a_line_once_even_when_hunks_overlap() {
        // Hunks expanded to their enclosing scope can cover the same line
        // twice; the reviewer wrote one note and expects one block.
        let first = sample_hunk();
        let second = sample_hunk();
        let mut store = CommentStore::default();
        note_at(&mut store, anchor("a.rs", LineSide::New, 10), "once", "c");

        let (text, count) =
            review_text("", &store, &[section("a.rs", &[first, second])]).expect("one comment");
        assert_eq!(count, 1);
        assert_eq!(text.matches("\nonce\n").count(), 1);
    }

    #[test]
    fn review_text_marks_a_comment_outdated_when_the_line_changed_in_place() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("a.rs", LineSide::New, 10),
            "keep the intent",
            "fn old_name() {",
        );

        let (text, _) = review_text("", &store, &[section("a.rs", &[hunk])]).unwrap();

        assert!(text.contains("a.rs:10 (outdated)\n+ fn old_name() {\nkeep the intent\n"));
    }

    #[test]
    fn review_text_keeps_comments_whose_line_left_the_diff() {
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

        let (text, count) = review_text("", &store, &[section("a.rs", &[hunk])]).unwrap();
        assert_eq!(count, 2);
        assert!(text.find("\nanchored\n").unwrap() < text.find("\norphan\n").unwrap());
        assert!(text.contains("z.rs:99 (outdated)"));
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

    fn numbered_section<'a>(
        path: &str,
        hunks: &'a [Hunk],
        numbering: Numbering,
    ) -> DiffSection<'a> {
        DiffSection {
            numbering,
            ..section(path, hunks)
        }
    }

    #[test]
    fn numbering_maps_each_side_to_its_file_version() {
        let hunk = sample_hunk();
        let sides = |numbering| {
            numbered_lines(&hunk, numbering)
                .map(|line| (line.side, line.number))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sides(Numbering::STAGED),
            [
                (LineSide::Index, 10),
                (LineSide::Old, 11),
                (LineSide::Index, 11)
            ]
        );
        assert_eq!(
            sides(Numbering::UNSTAGED),
            [
                (LineSide::New, 10),
                (LineSide::Index, 11),
                (LineSide::New, 11)
            ]
        );
    }

    #[test]
    fn reanchor_keeps_a_comment_that_another_diff_of_the_file_still_shows() {
        let mut store = CommentStore::default();
        let index_line = anchor("a.rs", LineSide::Index, 11);
        note_at(&mut store, index_line.clone(), "note", "let x = 2;");
        // The staged diff shows index line 11; the unstaged diff removes it
        // and adds an identical line elsewhere, which must not attract it.
        let staged = [sample_hunk()];
        let unstaged = [Hunk {
            old_start: 11,
            new_start: 11,
            lines: vec![
                diff_line(LineKind::Removed, "let x = 2;"),
                diff_line(LineKind::Added, "let x = 3;"),
            ],
            ancestors: Vec::new(),
        }];
        reanchor(
            &mut store,
            &[
                numbered_section("a.rs", &staged, Numbering::STAGED),
                numbered_section("a.rs", &unstaged, Numbering::UNSTAGED),
            ],
        );
        assert_eq!(store.note(&index_line), Some("note"));
    }

    #[test]
    fn reanchor_moves_an_index_line_to_the_worktree_when_the_file_is_fully_staged() {
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("a.rs", LineSide::Index, 11),
            "note",
            "let x = 2;",
        );
        // Unstaging everything leaves one HEAD → worktree diff.
        let net = [Hunk {
            old_start: 11,
            new_start: 12,
            lines: vec![diff_line(LineKind::Added, "let x = 2;")],
            ancestors: Vec::new(),
        }];
        reanchor(&mut store, &[section("a.rs", &net)]);
        assert_eq!(store.note(&anchor("a.rs", LineSide::New, 12)), Some("note"));
    }

    #[test]
    fn review_text_is_none_without_comments() {
        let hunk = sample_hunk();
        let store = CommentStore::default();
        assert!(review_text("", &store, &[section("a.rs", &[hunk])]).is_none());
    }

    #[test]
    fn review_text_relativizes_absolute_paths_against_the_working_directory() {
        let hunk = sample_hunk();
        let mut store = CommentStore::default();
        note_at(
            &mut store,
            anchor("/repo/src/a.rs", LineSide::New, 10),
            "note",
            "c",
        );
        let (text, _) = review_text("/repo", &store, &[section("/repo/src/a.rs", &[hunk])])
            .expect("one comment");
        assert!(text.contains("src/a.rs:10"));
    }

    #[test]
    fn relativize_leaves_paths_outside_the_working_directory_alone() {
        assert_eq!(relativize("/repo", "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relativize("/repo/", "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relativize("/repo", "/other/a.rs"), "/other/a.rs");
        assert_eq!(relativize("", "src/a.rs"), "src/a.rs");
    }
}
