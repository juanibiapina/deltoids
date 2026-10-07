//! Diff hunks with tree-sitter scope context.
//!
//! [`Diff::compute`] diffs two texts line by line and groups the changes
//! into [`Hunk`]s. When the language parses, each hunk's context grows to
//! the scope it changes (function, class, statement, literal) and carries
//! the scope's ancestor chain as its breadcrumb; otherwise hunks get the
//! standard 3 lines of context. Every changed line appears in exactly one
//! hunk, and hunks never overlap.

use std::ops::Range;

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
        let hunks = build_hunks(&snapshot, original, updated, language);
        Diff {
            snapshot,
            hunks,
            language,
            highlight,
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

/// Build the hunk list for a diff. A new file (empty original) is all
/// additions, so scope context would only add misleading breadcrumbs; it
/// and files that do not parse get plain context.
fn build_hunks(
    snapshot: &Snapshot,
    original: &str,
    updated: &str,
    language: Option<Language>,
) -> Vec<Hunk> {
    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = updated.lines().collect();
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
    hunks::build(snapshot.ops(), &old_lines, &new_lines, &expansion)
}
