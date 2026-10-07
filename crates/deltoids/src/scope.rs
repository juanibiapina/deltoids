//! Diff hunks with tree-sitter scope context.
//!
//! [`Diff::compute`] diffs two texts line by line and groups the changes
//! into [`Hunk`]s. When the language parses, each hunk's context grows to
//! the scope it changes (function, class, statement, literal) and carries
//! the scope's ancestor chain as its breadcrumb; otherwise hunks get the
//! standard 3 lines of context. Every changed line appears in exactly one
//! hunk, and hunks never overlap. [`Diff::expand`] grows one hunk's context
//! by one scope level on request, and [`Diff::shrink`] undoes that.

use std::ops::Range;
use std::sync::Arc;

use crate::Language;
use crate::engine::Snapshot;
use crate::syntax::ParsedFile;
use expansion::Expansion;
use serde::{Deserialize, Serialize};

mod expansion;
mod hunks;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Added,
    Removed,
    Context,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
    pub ancestors: Vec<ScopeNode>,
}

/// One run of consecutive lines inside a `Hunk`.
///
/// `Context` is a single unchanged line. `Change` is a maximal run of
/// consecutive `Added`/`Removed` lines, ready to feed into intraline
/// emphasis pairing. Splitting on context boundaries matches what
/// renderers need: context lines render directly, change runs render as
/// a paired subhunk.
#[derive(Debug, Clone, Copy)]
pub enum HunkRun<'a> {
    Context(&'a DiffLine),
    Change(&'a [DiffLine]),
}

impl Hunk {
    /// Walk the hunk as a sequence of context singletons and maximal
    /// change runs.
    pub fn runs(&self) -> impl Iterator<Item = HunkRun<'_>> {
        HunkRunsIter {
            lines: &self.lines,
            index: 0,
        }
    }
}

struct HunkRunsIter<'a> {
    lines: &'a [DiffLine],
    index: usize,
}

impl<'a> Iterator for HunkRunsIter<'a> {
    type Item = HunkRun<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.lines.len() {
            return None;
        }
        let line = &self.lines[self.index];
        if matches!(line.kind, LineKind::Context) {
            self.index += 1;
            return Some(HunkRun::Context(line));
        }
        let start = self.index;
        while self.index < self.lines.len()
            && !matches!(self.lines[self.index].kind, LineKind::Context)
        {
            self.index += 1;
        }
        Some(HunkRun::Change(&self.lines[start..self.index]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeNode {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

impl ScopeNode {
    /// 0-based, end-exclusive line span (`start_line`/`end_line` are
    /// 1-based and inclusive).
    pub(crate) fn lines(&self) -> Range<usize> {
        self.start_line.saturating_sub(1)..self.end_line
    }
}

/// A diff enriched with tree-sitter scope information.
///
/// Use `Diff::compute()` to create a diff from original and updated content.
/// The diff provides both raw diff text and structured hunks with
/// ancestor scope chains.
#[derive(Debug, Clone)]
pub struct Diff {
    snapshot: Snapshot,
    hunks: Vec<Hunk>,
    language: Option<Language>,
    highlight: Option<String>,
    original: Arc<str>,
    updated: Arc<str>,
    /// Old-line ranges the reader asked to see through [`Diff::expand`].
    widened: Vec<Range<usize>>,
}

impl Diff {
    /// Compute a diff between original and updated content.
    ///
    /// Parses both sides with tree-sitter (when the language is supported)
    /// to grow each hunk's context to the scope it changes and to name the
    /// hunk with its ancestor scope chain. The `text()` method returns
    /// standard 3-line context.
    pub fn compute(original: &str, updated: &str, path: &str) -> Self {
        let snapshot = Snapshot::compute(original, updated);
        let language = Language::detect(path, updated).or_else(|| Language::detect(path, original));
        let highlight = Language::detect_highlight_name(path, updated)
            .or_else(|| Language::detect_highlight_name(path, original));
        let hunks = with_expansion(original, updated, language, |expansion| {
            build_hunks(&snapshot, original, updated, expansion, &[])
        });
        Diff {
            snapshot,
            hunks,
            language,
            highlight,
            original: Arc::from(original),
            updated: Arc::from(updated),
            widened: Vec::new(),
        }
    }

    /// This diff with hunk `index`'s context grown one scope level
    /// outward, merged with any hunks it now overlaps.
    ///
    /// The next level is the innermost enclosing scope (function, class,
    /// module, …) larger than the hunk's old lines; failing that, the
    /// innermost larger syntax node (a Markdown section, a data literal);
    /// failing that, the whole file. Files that do not parse grow by 20
    /// lines on each side. Returns `None` when the hunk already shows the
    /// whole old file or `index` is out of range.
    pub fn expand(&self, index: usize) -> Option<Diff> {
        let hunk = self.hunks.get(index)?;
        let (original, updated) = (&*self.original, &*self.updated);
        with_expansion(original, updated, self.language, |expansion| {
            let level = expansion.next_level(
                old_span(hunk),
                changed_span(hunk),
                original.lines().count(),
            )?;
            let mut widened = self.widened.clone();
            widened.push(level);
            Some(self.rebuilt(expansion, widened))
        })
    }

    /// This diff with the latest [`Diff::expand`] that reached hunk
    /// `index` undone. A hunk that expansion merged splits back apart.
    /// Returns `None` when no expansion reached the hunk or `index` is out
    /// of range.
    pub fn shrink(&self, index: usize) -> Option<Diff> {
        let changed = changed_span(self.hunks.get(index)?);
        let latest = self
            .widened
            .iter()
            .rposition(|range| range.start <= changed.end && changed.start <= range.end)?;
        let mut widened = self.widened.clone();
        widened.remove(latest);
        with_expansion(&self.original, &self.updated, self.language, |expansion| {
            Some(self.rebuilt(expansion, widened))
        })
    }

    /// This diff with its hunks rebuilt to show `widened`.
    fn rebuilt(&self, expansion: &Expansion<'_>, widened: Vec<Range<usize>>) -> Diff {
        Diff {
            hunks: build_hunks(
                &self.snapshot,
                &self.original,
                &self.updated,
                expansion,
                &widened,
            ),
            snapshot: self.snapshot.clone(),
            language: self.language,
            highlight: self.highlight.clone(),
            original: Arc::clone(&self.original),
            updated: Arc::clone(&self.updated),
            widened,
        }
    }

    /// Returns the diff text with standard 3-line context.
    pub fn text(&self) -> &str {
        self.snapshot.unified_text()
    }

    /// Returns the enriched hunks.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// Returns the detected language used for tree-sitter scope expansion.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Returns the detected syntect syntax name used as the highlight key.
    pub fn highlight(&self) -> Option<&str> {
        self.highlight.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Hunk construction
// ---------------------------------------------------------------------------

/// Old lines a hunk shows: its context and removed lines. Empty at the
/// insertion point for a hunk that only adds lines.
fn old_span(hunk: &Hunk) -> Range<usize> {
    let start = hunk.old_start - 1;
    let len = hunk
        .lines
        .iter()
        .filter(|line| line.kind != LineKind::Added)
        .count();
    start..start + len
}

/// Old lines from the hunk's first change to its last removed line. Empty
/// at the insertion point for a hunk that only adds lines.
fn changed_span(hunk: &Hunk) -> Range<usize> {
    let mut old_at = hunk.old_start - 1;
    let mut changed: Option<Range<usize>> = None;
    for line in &hunk.lines {
        match line.kind {
            LineKind::Context => old_at += 1,
            LineKind::Removed => {
                let start = changed.map_or(old_at, |range| range.start);
                changed = Some(start..old_at + 1);
                old_at += 1;
            }
            LineKind::Added => {
                changed.get_or_insert(old_at..old_at);
            }
        }
    }
    changed.unwrap_or(old_at..old_at)
}

/// Build the hunk list for a diff, showing at least the `widened` old
/// lines around the changes inside them.
fn build_hunks(
    snapshot: &Snapshot,
    original: &str,
    updated: &str,
    expansion: &Expansion<'_>,
    widened: &[Range<usize>],
) -> Vec<Hunk> {
    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = updated.lines().collect();
    hunks::build(snapshot.ops(), &old_lines, &new_lines, expansion, widened)
}

/// Run `f` with the diff's context policy. A new file (empty original) is
/// all additions, so scope context would only add misleading breadcrumbs;
/// it and files that do not parse get plain context.
fn with_expansion<R>(
    original: &str,
    updated: &str,
    language: Option<Language>,
    f: impl FnOnce(&Expansion<'_>) -> R,
) -> R {
    let parsed = language
        .filter(|_| !original.is_empty())
        .and_then(|language| {
            Some((
                ParsedFile::parse_as(language, original)?,
                ParsedFile::parse_as(language, updated)?,
            ))
        });
    let expansion = match &parsed {
        Some((old, new)) => Expansion::Scoped { old, new },
        None => Expansion::Plain,
    };
    f(&expansion)
}
