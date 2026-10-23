//! One file's block in a summary: what its change did to the file's
//! definitions, one row per changed definition with its highlighted
//! signature; for a file without definitions (plain text, a language
//! without tree-sitter support), one row per hunk naming the scope it
//! changes; for a file without a text diff, a note. The render cache
//! draws these in the background (see [`super::render::BlockKind`]).
//!
//! Every row leads somewhere: opening it shows the file's diff at the
//! row's first changed line.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::parse::FileDiff;
use deltoids::render_tui::{render_code_line, rgb_to_color};
use deltoids::{
    Aspects, DefinitionChange, DefinitionDiff, Diff, Hunk, LineKind, ScopeNode, Status, Theme,
    TopLevelChange,
};

use crate::cli::browse::comments::{CommentAnchor, Numbering, numbered_lines};
use crate::cli::browse::diff_cursor::{DiffRow, LinePlace, Opens};
use crate::cli::browse::text::{display_width, shorten_right};
use crate::sidebar::display_path;

use super::model::FileBody;

const INDENT: &str = "  ";

/// The block of the file \`file\` whose diff is \`body\`, \`width\` columns wide.
pub(super) fn render_block(
    file: &FileDiff,
    body: &FileBody,
    numbering: Numbering,
    width: usize,
    theme: &Theme,
) -> Vec<DiffRow> {
    let mut block = Block {
        path: display_path(file).to_string(),
        numbering,
        width,
        theme,
        rows: Vec::new(),
        highlight: None,
    };
    match body {
        FileBody::Diff(diff) => block.diff(diff),
        FileBody::Symlink(_) => block.note("symlink"),
        FileBody::Binary => block.note("binary"),
        FileBody::StatusOnly => block.note("staged and unstaged changes cancel out"),
        FileBody::Submodule { .. } => block.note("submodule"),
    }
    block.rows
}

struct Block<'a> {
    path: String,
    numbering: Numbering,
    width: usize,
    theme: &'a Theme,
    rows: Vec<DiffRow>,
    highlight: Option<String>,
}

impl Block<'_> {
    fn diff(&mut self, diff: &Diff) {
        if diff.hunks().is_empty() {
            return self.note("no text changes");
        }
        self.highlight = diff.highlight().map(str::to_string);
        match diff.definitions() {
            Some(definitions) if !definitions.is_empty() => self.definitions(definitions),
            _ => {
                for hunk in diff.hunks() {
                    self.hunk(hunk);
                }
            }
        }
    }

    fn definitions(&mut self, diff: &DefinitionDiff) {
        if diff.has_errors {
            self.rows.push(DiffRow::plain(
                self.muted("syntax errors: what changed is a best guess", INDENT),
            ));
        }
        for change in &diff.changes {
            self.change(change, 1);
        }
        if let Some(top) = &diff.top_level {
            self.top_level(top);
        }
    }

    /// A row leading to \`at\` in this file.
    fn push(&mut self, line: Line<'static>, at: Option<CommentAnchor>) {
        let place = LinePlace::summary(&self.path, self.rows.len() + 1);
        let opens = Opens {
            path: self.path.clone(),
            at,
        };
        self.rows.push(DiffRow::summary(line, place, opens));
    }

    fn note(&mut self, text: &str) {
        let line = self.muted(text, INDENT);
        self.push(line, None);
    }

    fn muted(&self, text: &str, indent: &str) -> Line<'static> {
        let room = self.width.saturating_sub(indent.len());
        Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(shorten_right(text, room), self.style(self.theme.muted)),
        ])
    }

    fn style(&self, rgb: (u8, u8, u8)) -> Style {
        Style::default().fg(rgb_to_color(rgb))
    }

    /// The anchor of 1-based \`line\` on the old or the new side.
    fn anchor(&self, new: bool, line: usize) -> CommentAnchor {
        let side = if new {
            self.numbering.new
        } else {
            self.numbering.old
        };
        CommentAnchor {
            path: self.path.clone(),
            side,
            line,
        }
    }

    /// \`{indent}{marker} {signature}  {what changed} ······ +a -r\`, then the
    /// old signature when it changed, then the definitions inside it.
    fn change(&mut self, change: &DefinitionChange, depth: usize) {
        let indent = INDENT.repeat(depth);
        let (marker, color) = marker(&change.status, self.theme);
        let place = change.new.as_ref().or(change.old.as_ref());
        let signature = place.map_or(change.name.as_str(), |place| place.signature.as_str());
        let counts = counts(change.lines, self.theme);
        let counts_width: usize = counts.iter().map(|span| display_width(&span.content)).sum();
        // The signature first; what changed gets the room left over.
        let room = self
            .width
            .saturating_sub(indent.len() + 2 + 2 + counts_width)
            .max(8);
        let shown = shorten_right(signature, room);
        let left = room.saturating_sub(display_width(&shown) + 2);
        let note = match describe(change) {
            note if left >= 4 => shorten_right(&note, left),
            _ => String::new(),
        };
        let mut spans = vec![
            Span::raw(indent.clone()),
            Span::styled(
                marker.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        spans.extend(render_code_line(
            &shown,
            self.highlight.as_deref(),
            self.theme,
        ));
        if !note.is_empty() {
            spans.push(Span::styled(
                format!("  {note}"),
                self.style(self.theme.muted),
            ));
        }
        let used: usize = spans.iter().map(|span| display_width(&span.content)).sum();
        spans.push(Span::raw(
            " ".repeat(self.width.saturating_sub(used + counts_width)),
        ));
        spans.extend(counts);
        let at = self.opens_at(change);
        self.push(Line::from(spans), at.clone());
        if change.aspects.signature
            && let (Some(old), Some(_)) = (&change.old, &change.new)
        {
            let text = format!("was {}", old.signature);
            let line = self.muted(&text, &format!("{indent}    "));
            self.push(line, at);
        }
        for child in &change.children {
            self.change(child, depth + 1);
        }
    }

    /// Where opening a change goes: its first changed line, or where it
    /// starts, in the new file (the old one for a removed definition).
    fn opens_at(&self, change: &DefinitionChange) -> Option<CommentAnchor> {
        match (&change.new, &change.old) {
            (Some(new), _) => {
                let line = new.first_change.unwrap_or(*new.lines.start());
                Some(self.anchor(true, line))
            }
            (None, Some(old)) => {
                let line = old.first_change.unwrap_or(*old.lines.start());
                Some(self.anchor(false, line))
            }
            (None, None) => None,
        }
    }

    fn top_level(&mut self, top: &TopLevelChange) {
        let (marker, color) = ("~", rgb_to_color(self.theme.status_modified));
        let what = aspect_names(&top.aspects).join(", ");
        let counts = counts(top.lines, self.theme);
        let mut spans = vec![
            Span::raw(INDENT),
            Span::styled(
                marker,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("top level", Style::default().add_modifier(Modifier::ITALIC)),
            Span::styled(format!("  {what}"), self.style(self.theme.muted)),
        ];
        let used: usize = spans.iter().map(|span| display_width(&span.content)).sum();
        let counts_width: usize = counts.iter().map(|span| display_width(&span.content)).sum();
        spans.push(Span::raw(
            " ".repeat(self.width.saturating_sub(used + counts_width)),
        ));
        spans.extend(counts);
        let at = match (top.new_first_change, top.old_first_change) {
            (Some(line), _) => Some(self.anchor(true, line)),
            (None, Some(line)) => Some(self.anchor(false, line)),
            (None, None) => None,
        };
        self.push(Line::from(spans), at);
    }

    /// \`  impl DiffPane › fn new ······ +1\`: a hunk of a file without
    /// definitions, named by the scope it sits in.
    fn hunk(&mut self, hunk: &Hunk) {
        let kinds = || hunk.lines.iter().map(|line| &line.kind);
        let added = kinds().filter(|kind| **kind == LineKind::Added).count();
        let removed = kinds().filter(|kind| **kind == LineKind::Removed).count();
        let counts = counts((added, removed), self.theme);
        let counts_width: usize = counts.iter().map(|span| display_width(&span.content)).sum();
        let room = self.width.saturating_sub(INDENT.len() + 2 + counts_width);
        let scope = shorten_right(&scope_label(hunk), room);
        let used = INDENT.len() + display_width(&scope);
        let mut spans = vec![Span::raw(INDENT), Span::raw(scope)];
        spans.push(Span::raw(
            " ".repeat(self.width.saturating_sub(used + counts_width)),
        ));
        spans.extend(counts);
        let at = numbered_lines(hunk, self.numbering)
            .find(|line| *line.kind != LineKind::Context)
            .map(|line| CommentAnchor {
                path: self.path.clone(),
                side: line.side,
                line: line.number,
            });
        self.push(Line::from(spans), at);
    }
}

/// The marker and its color for a status.
fn marker(status: &Status, theme: &Theme) -> (&'static str, ratatui::style::Color) {
    let (marker, rgb) = match status {
        Status::Added => ("+", theme.status_added),
        Status::Removed => ("-", theme.status_deleted),
        Status::Modified => ("~", theme.status_modified),
        Status::Renamed { .. } => ("\u{2192}", theme.status_partial),
        Status::Moved { .. } => ("\u{21b7}", theme.status_partial),
        Status::Container => (" ", theme.muted),
    };
    (marker, rgb_to_color(rgb))
}

/// What changed, in words: \`body, docs\`, \`renamed from old\`, \`moved
/// from Server\`.
fn describe(change: &DefinitionChange) -> String {
    let aspects = aspect_names(&change.aspects).join(", ");
    let joined = |lead: String| {
        if aspects.is_empty() {
            lead
        } else {
            format!("{lead}; {aspects}")
        }
    };
    match &change.status {
        Status::Renamed { from } => joined(format!("renamed from {from}")),
        Status::Moved { from: Some(from) } if from.is_empty() => {
            joined("moved from top level".to_string())
        }
        Status::Moved { from: Some(from) } => {
            joined(format!("moved from {}", from.join(" \u{203a} ")))
        }
        Status::Moved { from: None } => joined("moved".to_string()),
        Status::Modified => aspects,
        Status::Added | Status::Removed | Status::Container => String::new(),
    }
}

fn aspect_names(aspects: &Aspects) -> Vec<&'static str> {
    [
        (aspects.signature, "signature"),
        (aspects.attributes, "attributes"),
        (aspects.body, "body"),
        (aspects.docs, "docs"),
        (aspects.comments, "comments"),
        (aspects.imports, "imports"),
        (aspects.formatting, "formatting"),
    ]
    .into_iter()
    .filter(|(set, _)| *set)
    .map(|(_, name)| name)
    .collect()
}

/// \`+a -r\`, leaving out a zero side.
fn counts((added, removed): (usize, usize), theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if added > 0 {
        spans.push(Span::styled(
            format!("+{added}"),
            Style::default().fg(rgb_to_color(theme.status_added)),
        ));
    }
    if removed > 0 {
        if added > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("-{removed}"),
            Style::default().fg(rgb_to_color(theme.status_deleted)),
        ));
    }
    spans
}

/// The hunk's two innermost scopes as their source reads them
/// (\`impl DiffPane › fn new\`), or the lines it covers when it sits in
/// no scope.
fn scope_label(hunk: &Hunk) -> String {
    let start = hunk.ancestors.len().saturating_sub(2);
    let scopes: Vec<String> = hunk.ancestors[start..]
        .iter()
        .map(scope_text)
        .filter(|text| !text.is_empty())
        .collect();
    if !scopes.is_empty() {
        return scopes.join(" \u{203a} ");
    }
    let lines = hunk
        .lines
        .iter()
        .filter(|line| line.kind != LineKind::Removed)
        .count();
    match lines {
        0 | 1 => format!("line {}", hunk.new_start),
        _ => format!(
            "lines {}\u{2013}{}",
            hunk.new_start,
            hunk.new_start + lines - 1
        ),
    }
}

/// A scope's first source line, short: up to and including its name, so
/// \`fn render(&self) -> Line {\` reads \`fn render\`, without a leading
/// \`pub\` or \`export\`. A scope without a name keeps its line, minus an
/// opening \`{\`.
fn scope_text(scope: &ScopeNode) -> String {
    let text = scope.text.trim();
    let text = match (!scope.name.is_empty())
        .then(|| text.find(scope.name.as_str()))
        .flatten()
    {
        Some(at) => &text[..at + scope.name.len()],
        None => text.trim_end_matches('{').trim_end(),
    };
    strip_visibility(text).to_string()
}

fn strip_visibility(text: &str) -> &str {
    let mut text = text;
    loop {
        let rest = if let Some(rest) = text.strip_prefix("pub(") {
            rest.find(") ").map(|end| &rest[end + 2..])
        } else {
            ["pub ", "export default ", "export "]
                .iter()
                .find_map(|prefix| text.strip_prefix(prefix))
        };
        match rest {
            Some(rest) => text = rest,
            None => return text,
        }
    }
}
