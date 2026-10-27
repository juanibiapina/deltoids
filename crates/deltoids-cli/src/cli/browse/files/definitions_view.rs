//! One file's block in a summary or the definitions view: what its change
//! did to the file's definitions, one row per changed definition with its
//! highlighted signature. The definitions view also shows the unchanged
//! definitions around the changes, as a diff shows context lines:
//! all of a small level (the file, or a changed definition's members), and
//! a few on each side of each change in a larger one. A level with
//! definitions of two or more visibilities lists them in groups under a
//! heading, public first. A file without
//! definitions (plain text, a language without tree-sitter support) gets
//! one row per hunk naming the scope it changes; a file without a text
//! diff, a note. The render cache draws these in the background (see
//! [`super::render::BlockKind`]).
//!
//! Every definition row leads somewhere: opening it shows the file's diff
//! at the row's first changed line, or where an unchanged definition
//! starts.

use std::collections::BTreeMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::parse::FileDiff;
use deltoids::render_tui::{render_code_line, rgb_to_color};
use deltoids::{
    Aspects, DefinitionChange, DefinitionDiff, Diff, Hunk, LineKind, ScopeNode, Status, Theme,
    TopLevelChange, Visibility,
};

use crate::cli::browse::comments::{CommentAnchor, Numbering, numbered_lines};
use crate::cli::browse::diff_cursor::{DiffRow, LinePlace, Opens};
use crate::cli::browse::text::{display_width, shorten_right};
use crate::sidebar::display_path;

use super::model::FileBody;

const INDENT: &str = "  ";
/// A level (the file, or a changed definition's members) with at most this
/// many definitions shows them all, as a hunk shows a whole small scope.
const WHOLE_LEVEL: usize = 40;
/// In a larger level, the unchanged definitions shown on each side of a
/// changed one.
const AROUND: usize = 3;

/// How much of a file's definitions a block shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Detail {
    /// The changed definitions alone: a summary's block.
    Changes,
    /// The changed definitions among the unchanged ones around them: the
    /// definitions view.
    Outline,
}

/// The block of the file `file` whose diff is `body`, `width` columns wide.
pub(super) fn render_block(
    file: &FileDiff,
    body: &FileBody,
    numbering: Numbering,
    width: usize,
    theme: &Theme,
    detail: Detail,
) -> Vec<DiffRow> {
    let mut block = Block {
        path: display_path(file).to_string(),
        numbering,
        width,
        theme,
        detail,
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
    detail: Detail,
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
        let items = file_items(&diff.definitions, &diff.top_level);
        self.level(&items, 1);
    }

    /// One level of definitions, `depth` deep: in groups by visibility
    /// under a heading each, public first, when [`groups`] makes some; in
    /// file order otherwise.
    fn level(&mut self, items: &[Item<'_>], depth: usize) {
        let Some(groups) = groups(items, self.detail) else {
            return self.run(items, depth);
        };
        for (index, (group, members)) in groups.into_iter().enumerate() {
            if index > 0 {
                self.space();
            }
            if let Some(label) = group.label() {
                let line = self.muted(label, &INDENT.repeat(depth));
                self.rows.push(DiffRow::plain(line));
            }
            self.run(&members, depth);
        }
    }

    /// A run of a level's items, `depth` deep: the changed ones, and in
    /// an outline the unchanged ones around them, with the rest counted in
    /// `⋯ N more` rows. A definition that shows the ones inside it reads
    /// as a block: a blank row sets it apart from its neighbours.
    fn run(&mut self, items: &[Item<'_>], depth: usize) {
        let shown = match self.detail {
            Detail::Changes => items.iter().map(|item| item.is_changed()).collect(),
            Detail::Outline => context(items),
        };
        let start = self.rows.len();
        let mut hidden = 0;
        let mut after_block = false;
        for (&item, shown) in items.iter().zip(shown) {
            if !shown {
                hidden += 1;
                continue;
            }
            if self.folds(hidden) {
                self.separate(after_block, start);
                self.gap(hidden, depth);
                after_block = false;
            }
            hidden = 0;
            let block = matches!(item, Item::Definition(definition) if self.opens_up(definition));
            self.separate(after_block || block, start);
            match item {
                Item::Definition(definition) if definition.is_changed() => {
                    self.change(definition, depth);
                }
                Item::Definition(definition) => self.unchanged(definition, depth),
                Item::TopLevel(top) => self.top_level(top),
            }
            after_block = block;
        }
        if self.folds(hidden) {
            self.separate(after_block, start);
            self.gap(hidden, depth);
        }
    }

    /// Whether `hidden` left-out definitions get a `⋯ N more` row.
    fn folds(&self, hidden: usize) -> bool {
        hidden > 0 && self.detail == Detail::Outline
    }

    /// Whether `definition` shows definitions inside it.
    fn opens_up(&self, definition: &DefinitionChange) -> bool {
        definition.is_changed()
            && match self.detail {
                Detail::Changes => definition.children.iter().any(DefinitionChange::is_changed),
                Detail::Outline => !definition.children.is_empty(),
            }
    }

    /// A blank row when `wanted` and the level, which started at row
    /// `start`, already has rows.
    fn separate(&mut self, wanted: bool, start: usize) {
        if wanted && self.rows.len() > start {
            self.space();
        }
    }

    /// A blank row, unless the block is empty or already ends in one.
    fn space(&mut self) {
        if self.rows.last().is_some_and(|row| row.line.width() > 0) {
            self.rows.push(DiffRow::plain(Line::default()));
        }
    }

    /// `{indent}  {signature}`: an unchanged definition, opening where it
    /// starts.
    fn unchanged(&mut self, definition: &DefinitionChange, depth: usize) {
        let Some(place) = definition.new.as_ref() else {
            return;
        };
        let indent = INDENT.repeat(depth);
        let room = self.width.saturating_sub(indent.len() + 2).max(8);
        let shown = shorten_right(&place.signature, room);
        let mut spans = vec![Span::raw(indent), Span::raw("  ")];
        spans.extend(render_code_line(
            &shown,
            self.highlight.as_deref(),
            self.theme,
        ));
        let at = self.anchor(true, *place.lines.start());
        self.push(Line::from(spans), Some(at));
    }

    /// `{indent}  ⋯ N more`: unchanged definitions an outline leaves out.
    fn gap(&mut self, hidden: usize, depth: usize) {
        if !self.folds(hidden) {
            return;
        }
        let indent = format!("{}  ", INDENT.repeat(depth));
        let line = self.muted(&format!("\u{22ef} {hidden} more"), &indent);
        self.rows.push(DiffRow::plain(line));
    }

    /// A row leading to `at` in this file.
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

    /// The anchor of 1-based `line` on the old or the new side.
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

    /// `{indent}{marker} {signature}  {what changed} ······ +a -r`, then the
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
        if let Some(bg) = tint(change, self.theme) {
            for span in &mut spans {
                span.style = span.style.bg(bg);
            }
        }
        let at = self.opens_at(change);
        self.push(Line::from(spans), at.clone());
        if change.aspects.signature
            && let (Some(old), Some(_)) = (&change.old, &change.new)
        {
            let text = format!("was {}", old.signature);
            let line = self.muted(&text, &format!("{indent}    "));
            self.push(line, at);
        }
        let children: Vec<Item<'_>> = change.children.iter().map(Item::Definition).collect();
        self.level(&children, depth + 1);
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

    /// `  ~ top level  comments ······ +a -r`: a change to the code outside
    /// every definition, where it sits among them.
    fn top_level(&mut self, top: &TopLevelChange) {
        let (marker, color) = ("~", rgb_to_color(self.theme.status_modified));
        // Outside every definition, a body is just code.
        let what: Vec<&str> = aspect_names(&top.aspects)
            .into_iter()
            .map(|name| if name == "body" { "code" } else { name })
            .collect();
        let what = what.join(", ");
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

    /// `  impl DiffPane › fn new ······ +1`: a hunk of a file without
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
        Status::Container | Status::Unchanged => (" ", theme.muted),
    };
    (marker, rgb_to_color(rgb))
}

/// The background of a changed definition's row, following its marker:
/// the diff's added or deleted line background for an added or removed
/// definition, the theme's changed-definition background for any other
/// whose own lines changed. None when its own lines are unchanged.
fn tint(change: &DefinitionChange, theme: &Theme) -> Option<ratatui::style::Color> {
    let rgb = match change.status {
        Status::Added => theme.diff_added_bg,
        Status::Removed => theme.diff_deleted_bg,
        _ if change.lines != (0, 0) => theme.definition_changed_bg,
        _ => return None,
    };
    Some(rgb_to_color(rgb))
}

/// What changed, in words: `body, docs`, `renamed from old`, `moved
/// from Server`.
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
        Status::Added | Status::Removed | Status::Container | Status::Unchanged => String::new(),
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

/// One row of a level: a definition, or a change to the code outside
/// every definition (only the file's level has those).
#[derive(Clone, Copy)]
enum Item<'d> {
    Definition(&'d DefinitionChange),
    TopLevel(&'d TopLevelChange),
}

impl Item<'_> {
    fn is_changed(self) -> bool {
        match self {
            Item::Definition(definition) => definition.is_changed(),
            Item::TopLevel(_) => true,
        }
    }

    fn group(self) -> Group {
        match self {
            Item::Definition(definition) => match definition.visibility {
                Some(Visibility::Public) => Group::Public,
                Some(Visibility::Restricted) => Group::Restricted,
                Some(Visibility::Private) => Group::Private,
                None => Group::Other,
            },
            Item::TopLevel(_) => Group::TopLevel,
        }
    }
}

/// A level's groups, in the order they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Public,
    Restricted,
    Private,
    /// Definitions without visibility: a test, an `impl` of a type from
    /// another file.
    Other,
    /// Changes to the code outside every definition: their place among
    /// the definitions is lost once those are grouped.
    TopLevel,
}

impl Group {
    fn label(self) -> Option<&'static str> {
        match self {
            Group::Public => Some("public"),
            Group::Restricted => Some("restricted"),
            Group::Private => Some("private"),
            Group::Other => Some("other"),
            Group::TopLevel => None,
        }
    }

    fn is_visibility(self) -> bool {
        matches!(self, Group::Public | Group::Restricted | Group::Private)
    }
}

/// A level's items in groups, in file order within each, when the ones
/// it shows have two or more visibilities. A group with nothing to show
/// (no change, in a summary) is left out.
fn groups<'d>(items: &[Item<'d>], detail: Detail) -> Option<Vec<(Group, Vec<Item<'d>>)>> {
    let mut groups: BTreeMap<Group, Vec<Item<'d>>> = BTreeMap::new();
    for &item in items {
        groups.entry(item.group()).or_default().push(item);
    }
    groups.retain(|_, members| {
        members
            .iter()
            .any(|item| detail == Detail::Outline || item.is_changed())
    });
    let visibilities = groups.keys().filter(|group| group.is_visibility()).count();
    (visibilities >= 2).then(|| groups.into_iter().collect())
}

/// The file's level: its definitions, with each change to the code
/// outside them before the first definition that starts at or after it.
fn file_items<'d>(definitions: &'d [DefinitionChange], top: &'d [TopLevelChange]) -> Vec<Item<'d>> {
    let mut items = Vec::with_capacity(definitions.len() + top.len());
    let mut tops = top.iter().peekable();
    for definition in definitions {
        if let Some(new) = &definition.new {
            let start = *new.lines.start();
            while let Some(change) = tops.next_if(|change| change.at <= start) {
                items.push(Item::TopLevel(change));
            }
        }
        items.push(Item::Definition(definition));
    }
    items.extend(tops.map(Item::TopLevel));
    items
}

/// Which of a level's items an outline shows: all of a small level; in a
/// larger one, the changes and up to [`AROUND`] definitions on each side of
/// each.
fn context(items: &[Item<'_>]) -> Vec<bool> {
    if items.len() <= WHOLE_LEVEL {
        return vec![true; items.len()];
    }
    let mut shown = vec![false; items.len()];
    let changed = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.is_changed());
    for (index, _) in changed {
        let end = (index + AROUND + 1).min(items.len());
        shown[index.saturating_sub(AROUND)..end].fill(true);
    }
    shown
}

/// `+a -r`, leaving out a zero side.
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
/// (`impl DiffPane › fn new`), or the lines it covers when it sits in
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
/// `fn render(&self) -> Line {` reads `fn render`, without a leading
/// `pub` or `export`. A scope without a name keeps its line, minus an
/// opening `{`.
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
