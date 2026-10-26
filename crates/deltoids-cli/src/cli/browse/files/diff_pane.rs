//! Diff pane vertical slice: its state (a retained per-file line cache,
//! cursor, scroll), its scroll math, its key handling, and its render.
//! Files mode owns one pane per staging column and draws the one the
//! selection shows. Each draw passes the pane its [`PaneSpec`]: the
//! column's files in sidebar order and the selection's slice of them, so
//! this slice never reaches into the sidebar's fields.
//!
//! Rendering runs in a worker owned by [`DiffCache`]. A draw submits the
//! selection and uses ready blocks or cheap placeholders. The assembled
//! window survives cursor and focus changes; comments remain an overlay.

use std::collections::HashMap;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use deltoids::render_tui::{self, pane_block_with_title_line, pane_border_color, rgb_to_color};
use deltoids::{Hunk, LineKind, Theme, git};

use deltoids::parse::FileDiff;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::cli::browse::comment_view::{highlight_row, with_comments};
use crate::cli::browse::comments::{CommentAnchor, CommentStore, Numbering, numbered_lines};
use crate::cli::browse::diff_cursor::{
    Cursor, DiffRow, LinePlace, Opens, Step, keep_visible, restore_cursor, select_row, step_cursor,
};
use crate::cli::browse::diff_scrollbar;
use crate::cli::browse::mode::DrawBudget;
use crate::cli::browse::panel::draw_filter_flag;
use crate::cli::browse::text::{display_width, wrap_text};
use crate::sidebar::{
    FileMode, IconMode, ModeChange, RowNote, display_path, file_metadata, render_note, symlink_icon,
};

use super::model::{Column, FileBody, Model};
use super::overview;
use super::render::{BlockKind, DiffCache};
use super::stage_panes::{PaneSpec, PaneTitle};

pub(super) const SCROLL_STEP_SMALL: usize = 1;
pub(super) const SCROLL_STEP_LARGE: usize = 3;

/// The identity every retained block shares: its render `width` and the
/// selected `syntax_theme` (a `&'static` registry name). A change to
/// either invalidates the whole store, since each alters every block's
/// rendered rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheEpoch {
    pub(super) width: usize,
    pub(super) syntax_theme: &'static str,
}

/// A commit's header as `git show` prints it above the diff, ending in a
/// blank line: the full id, the author, the date in this machine's time
/// zone, and the whole message indented four columns, its wider lines
/// wrapped to `width`.
fn commit_header(commit: &git::Commit, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    const INDENT: &str = "    ";
    let mut lines = vec![
        Line::from(vec![
            Span::raw("commit "),
            Span::styled(
                commit.id.clone(),
                Style::default().fg(rgb_to_color(theme.status_modified)),
            ),
        ]),
        Line::raw(format!("Author: {} <{}>", commit.author, commit.email)),
        Line::raw(format!(
            "Date:   {}",
            git_date(commit.time, &TimeZone::system())
        )),
        Line::default(),
    ];
    let room = width.saturating_sub(INDENT.len()).max(1);
    for line in commit.message.trim_end().lines() {
        if line.trim().is_empty() {
            lines.push(Line::default());
        } else if display_width(line) <= room {
            lines.push(Line::raw(format!("{INDENT}{line}")));
        } else {
            let wrapped = wrap_text(line, room);
            lines.extend(
                wrapped
                    .into_iter()
                    .map(|part| Line::raw(format!("{INDENT}{part}"))),
            );
        }
    }
    lines.push(Line::default());
    lines
}

/// `Tue Oct 13 15:07:15 2026 +0200`: `time` (seconds since the epoch)
/// in `zone`, as git prints dates by default.
fn git_date(time: i64, zone: &TimeZone) -> String {
    Timestamp::from_second(time).map_or_else(
        |_| time.to_string(),
        |at| {
            at.to_zoned(zone.clone())
                .strftime("%a %b %-d %T %Y %z")
                .to_string()
        },
    )
}

/// Render one file's block: file header, an optional rename header, then
/// the body — either each hunk (blank-separated) for a text diff, or the
/// symlink view for a symlink change. The text-diff path carries the
/// syntax-highlighting cost (`render_hunk`) that lazy rendering defers.
///
/// Every row of a diff line is tagged with the [`CommentAnchor`] it
/// belongs to; saved comments are spliced in later by
/// [`crate::cli::browse::comment_view::with_comments`].
#[allow(clippy::too_many_arguments)]
pub(super) fn render_file_block(
    file: &FileDiff,
    body: &FileBody,
    numbering: Numbering,
    width: usize,
    theme: &Theme,
    keep_going: &impl Fn() -> bool,
    publish: &mut dyn FnMut(Vec<DiffRow>),
) -> Option<Vec<DiffRow>> {
    let mut render = FileRender {
        width,
        numbering,
        theme,
        keep_going,
        preview: FilePreview {
            publish,
            published: false,
        },
    };
    let path = display_path(file);
    // The file's name lives on the pane's border, and on a divider row
    // when the window shows several files (see `file_divider`).
    let mut rows: Vec<DiffRow> = Vec::new();
    // A type change renders as a content diff, but its note box stands in
    // for the per-hunk line-number box: render the note box, then the hunk
    // bodies without their own boxes (avoiding a second, redundant box).
    if let Some(note) = typechange_note(file, theme) {
        rows.push(DiffRow::plain(Line::from("")));
        rows.extend(note.into_iter().map(DiffRow::plain));
        match body {
            FileBody::Diff(diff) => {
                render.hunks(&mut rows, diff, path, false)?;
            }
            // A type change *into* a submodule (regular → submodule) has no
            // textual body; render the placeholder below the note box so the
            // pane is not empty.
            FileBody::Submodule {
                old_commit,
                new_commit,
            } => {
                rows.push(DiffRow::plain(Line::from("")));
                rows.push(DiffRow::plain(submodule_placeholder(
                    old_commit.as_deref(),
                    new_commit.as_deref(),
                    theme,
                )));
            }
            _ => {}
        }
        return Some(rows);
    }

    match body {
        // Mirrors `render_hunk_list`: a blank separator before each hunk.
        FileBody::Diff(diff) => {
            render.hunks(&mut rows, diff, path, true)?;
        }
        FileBody::Symlink(view) => {
            rows.push(DiffRow::plain(Line::from("")));
            rows.extend(
                render_tui::render_symlink(view, symlink_icon(IconMode::from_env()), theme)
                    .into_iter()
                    .map(DiffRow::plain),
            );
        }
        FileBody::Binary => {
            rows.push(DiffRow::plain(Line::from("")));
            rows.push(DiffRow::plain(Line::from(Span::styled(
                "Binary file (no textual diff)".to_string(),
                Style::default().fg(rgb_to_color(theme.muted)),
            ))));
        }
        FileBody::StatusOnly => {
            rows.push(DiffRow::plain(Line::from("")));
            rows.push(DiffRow::plain(Line::from(Span::styled(
                "Staged and unstaged changes cancel out relative to HEAD.",
                Style::default().fg(rgb_to_color(theme.muted)),
            ))));
        }
        FileBody::Submodule {
            old_commit,
            new_commit,
        } => {
            rows.push(DiffRow::plain(Line::from("")));
            rows.push(DiffRow::plain(submodule_placeholder(
                old_commit.as_deref(),
                new_commit.as_deref(),
                theme,
            )));
        }
    }

    Some(rows)
}

struct FileRender<'a> {
    width: usize,
    numbering: Numbering,
    theme: &'a Theme,
    keep_going: &'a dyn Fn() -> bool,
    preview: FilePreview<'a>,
}

impl FileRender<'_> {
    fn hunks(
        &mut self,
        rows: &mut Vec<DiffRow>,
        diff: &deltoids::Diff,
        path: &str,
        header: bool,
    ) -> Option<()> {
        let keep = self.keep_going;
        let keep_going = || keep();
        for (index, hunk) in diff.hunks().iter().enumerate() {
            rows.push(DiffRow::plain(Line::from("")));
            let start = rows.len();
            let enabled = !self.preview.published;
            let numbering = self.numbering;
            let mut publish_hunk = |partial: &[render_tui::HunkRow]| {
                self.preview
                    .hunk(rows, hunk, (index, numbering), path, partial)
            };
            let callback = if enabled {
                Some(&mut publish_hunk as render_tui::HunkPreview<'_>)
            } else {
                None
            };
            let rendered = if header {
                render_tui::render_hunk_rows_with_preview(
                    hunk,
                    diff.highlight(),
                    self.width,
                    self.theme,
                    &keep_going,
                    callback,
                )
            } else {
                render_tui::render_hunk_body_rows_with_preview(
                    hunk,
                    diff.highlight(),
                    self.width,
                    self.theme,
                    &keep_going,
                    callback,
                )
            }?;
            push_hunk_rows(rows, rendered, hunk, (path, index), self.numbering);
            if header && let Some(row) = rows.get_mut(start) {
                row.hunk_start = true;
            }
            self.preview.rows(rows);
        }
        Some(())
    }
}

struct FilePreview<'a> {
    publish: &'a mut dyn FnMut(Vec<DiffRow>),
    published: bool,
}

impl FilePreview<'_> {
    fn rows(&mut self, rows: &[DiffRow]) {
        if self.published || rows.len() < render_tui::PREVIEW_ROWS {
            return;
        }
        self.published = true;
        (self.publish)(rows[..render_tui::PREVIEW_ROWS].to_vec());
    }

    fn hunk(
        &mut self,
        prefix: &[DiffRow],
        hunk: &Hunk,
        (index, numbering): (usize, Numbering),
        path: &str,
        partial: &[render_tui::HunkRow],
    ) {
        if self.published {
            return;
        }
        let mut rows: Vec<_> = prefix
            .iter()
            .take(render_tui::PREVIEW_ROWS)
            .cloned()
            .collect();
        push_hunk_rows(&mut rows, partial.to_vec(), hunk, (path, index), numbering);
        self.rows(&rows);
    }
}

/// The comment anchor for every logical line of `hunk`, in line order.
fn hunk_anchors(hunk: &Hunk, path: &str, limit: usize, numbering: Numbering) -> Vec<CommentAnchor> {
    numbered_lines(hunk, numbering)
        .take(limit)
        .map(|line| CommentAnchor {
            path: path.to_string(),
            side: line.side,
            line: line.number,
        })
        .collect()
}

/// Push already-rendered `rendered` hunk rows, tagging each with the diff
/// line it belongs to. `place` is the hunk's `(file path, hunk)` position,
/// which with the line's index gives every diff line a unique identity
/// for the cursor. Comments are not drawn here: they are spliced in on
/// the way to the screen, so writing one never re-renders the file.
fn push_hunk_rows(
    rows: &mut Vec<DiffRow>,
    rendered: Vec<render_tui::HunkRow>,
    hunk: &Hunk,
    place: (&str, usize),
    numbering: Numbering,
) {
    let (file, hunk_index) = place;
    let limit = rendered
        .iter()
        .filter_map(|row| row.source_line)
        .max()
        .map_or(0, |index| index + 1);
    let anchors = hunk_anchors(hunk, file, limit, numbering);
    for row in rendered {
        let Some(index) = row.source_line else {
            rows.push(DiffRow::plain(row.line));
            continue;
        };
        rows.push(DiffRow::line_row(
            row.line,
            hunk.lines[index].kind.clone(),
            anchors[index].clone(),
            row.first_row.then(|| LinePlace {
                file: file.to_string(),
                hunk: hunk_index,
                index,
            }),
            row.last_row,
        ));
    }
}

/// A muted placeholder line for a submodule (gitlink) change: its body is
/// a commit OID, not text, so there is no diff to paint. Shows the short
/// old/new commits, tolerating a missing side (a submodule add shows only
/// the new commit, a delete only the old).
fn submodule_placeholder(
    old_commit: Option<&str>,
    new_commit: Option<&str>,
    theme: &Theme,
) -> Line<'static> {
    let short = |c: &str| c.chars().take(7).collect::<String>();
    let text = match (old_commit, new_commit) {
        (Some(o), Some(n)) => format!("Submodule {} \u{2192} {}", short(o), short(n)),
        (None, Some(n)) => format!("Submodule {}", short(n)),
        (Some(o), None) => format!("Submodule {}", short(o)),
        (None, None) => "Submodule (no textual diff)".to_string(),
    };
    Line::from(Span::styled(
        text,
        Style::default().fg(rgb_to_color(theme.muted)),
    ))
}

/// A breadcrumb-style box describing a type change (regular ↔ symlink ↔
/// submodule), shown above the diff body. A type change renders as an
/// ordinary content diff (old bytes removed, new bytes added), which alone
/// does not convey that the file *became* a symlink (or a regular file);
/// this box spells that out, e.g. `type change: regular file → symlink`,
/// matching the symlink view's breadcrumb box. `None` for every
/// non-type-change file (a plain edit, an exec-bit flip, etc.).
fn typechange_note(file: &FileDiff, theme: &Theme) -> Option<Vec<Line<'static>>> {
    let ModeChange::TypeChange { old, new } = file_metadata(file).mode_change? else {
        return None;
    };
    let description = format!(
        "type change: {} \u{2192} {}",
        typechange_label(old),
        typechange_label(new)
    );
    Some(render_tui::render_note_box(
        symlink_icon(IconMode::from_env()),
        &description,
        theme,
    ))
}

/// Human-readable file-kind label for the type-change note.
fn typechange_label(mode: FileMode) -> &'static str {
    match mode {
        FileMode::Regular => "regular file",
        FileMode::Executable => "executable",
        FileMode::Symlink => "symlink",
        FileMode::Submodule => "submodule",
        FileMode::Other => "unknown",
    }
}

fn label_hunks(
    rows: &mut [DiffRow],
    width: usize,
    theme: &Theme,
    label: impl Fn(usize) -> Option<RowNote>,
) {
    let starts: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].hunk_start).collect();
    for (hunk, start) in starts.into_iter().enumerate() {
        let Some(label) = label(hunk) else {
            continue;
        };
        let Some(bottom) = (start + 1..rows.len()).find(|&i| line_ends_with(&rows[i].line, '╯'))
        else {
            continue;
        };
        let note = render_note(&label, false, false, theme);
        let line = &mut rows[bottom - 1].line;
        let used = line.width() + note.width();
        if used + 2 > width {
            continue;
        }
        line.spans.push(Span::raw(" ".repeat(width - used)));
        line.spans.extend(note.spans);
    }
}

/// Collapse each hunk labelled low value to its line-number box, which
/// shows where it starts and its tag, plus one muted row saying it is
/// hidden. The body up to the next hunk's separator is left out.
fn hide_low_hunks(
    rows: Vec<DiffRow>,
    theme: &Theme,
    label: impl Fn(usize) -> Option<RowNote>,
) -> Vec<DiffRow> {
    let starts: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].hunk_start).collect();
    let mut hidden: Vec<(usize, usize, &'static str)> = Vec::new();
    for (hunk, &start) in starts.iter().enumerate() {
        let Some(note) = label(hunk).filter(|note| note.low) else {
            continue;
        };
        let end = starts.get(hunk + 1).map_or(rows.len(), |next| next - 1);
        let body = (start + 1..end)
            .find(|&i| line_ends_with(&rows[i].line, '╯'))
            .map_or(start, |bottom| bottom + 1);
        hidden.push((body, end, note.tag));
    }
    if hidden.is_empty() {
        return rows;
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut ranges = hidden.into_iter().peekable();
    let mut skip_until = 0;
    for (i, row) in rows.into_iter().enumerate() {
        if let Some(&(start, end, tag)) = ranges.peek()
            && i == start
        {
            ranges.next();
            skip_until = end;
            out.push(DiffRow::plain(Line::from(Span::styled(
                format!("{tag} hunk hidden"),
                Style::default().fg(rgb_to_color(theme.muted)),
            ))));
        }
        if i >= skip_until {
            out.push(row);
        }
    }
    out
}

fn line_ends_with(line: &Line<'_>, end: char) -> bool {
    line.spans
        .last()
        .is_some_and(|span| span.content.ends_with(end))
}

/// The row that starts a file in a window of several: a rule with the
/// file's name in it, `○` first for a visited file, `old → new` for a
/// rename.
fn file_divider(model: &Model, index: usize, width: usize, theme: &Theme) -> Line<'static> {
    let rule = Style::default().fg(rgb_to_color(theme.separator));
    let label = file_label(model, index);
    let used = 4 + display_width(&label);
    Line::from(vec![
        Span::styled("── ", rule),
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}", "─".repeat(width.saturating_sub(used))), rule),
    ])
}

/// How a file is named on the border and on its divider.
fn file_label(model: &Model, index: usize) -> String {
    let file = &model.files[index].file;
    let path = match &file.rename_from {
        Some(old) => format!("{old} \u{2192} {}", file.new_path),
        None => display_path(file).to_string(),
    };
    if model.is_visited(index) {
        format!("\u{25cb} {path}")
    } else {
        path
    }
}

/// The pane's title naming file `index`: `─path ─`, with the
/// staging tabs when the selection has both columns. The label shortens
/// from the left, `…/files/navigate.rs`, to stay within `budget` columns.
pub(super) fn file_heading(
    model: &Model,
    index: usize,
    title: PaneTitle,
    budget: usize,
    theme: &Theme,
) -> Line<'static> {
    named_heading(&file_label(model, index), title, budget, theme)
}

/// The pane's title over file `index`'s changed definitions:
/// `─src/a.rs · definitions ─`, shaped like [`file_heading`].
pub(super) fn definitions_heading(
    model: &Model,
    index: usize,
    title: PaneTitle,
    budget: usize,
    theme: &Theme,
) -> Line<'static> {
    let label = format!("{} \u{b7} definitions", file_label(model, index));
    named_heading(&label, title, budget, theme)
}

/// The pane's title over a directory's overview: `─src/cli/ ─`, shaped
/// like [`file_heading`].
pub(super) fn directory_heading(
    directory: &str,
    title: PaneTitle,
    budget: usize,
    theme: &Theme,
) -> Line<'static> {
    named_heading(directory, title, budget, theme)
}

fn named_heading(label: &str, title: PaneTitle, budget: usize, theme: &Theme) -> Line<'static> {
    let tabs = match title {
        PaneTitle::Both(shown) => Some(shown),
        _ => None,
    };
    let tabs_width = if tabs.is_some() { 22 } else { 0 };
    let room = budget.saturating_sub(3 + tabs_width);
    let mut spans = vec![
        Span::raw("─"),
        Span::styled(
            shorten_left(label, room),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(shown) = tabs {
        spans.push(Span::raw(" · "));
        spans.extend(column_tabs(shown, theme));
    }
    spans.push(Span::raw(" ─"));
    Line::from(spans)
}

/// `text` cut to `width` columns from the left, with `…` in front when
/// it was cut.
pub(super) fn shorten_left(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let w = display_width(&ch.to_string());
        if used + w > width {
            break;
        }
        used += w;
        kept.push(ch);
    }
    std::iter::once('\u{2026}')
        .chain(kept.into_iter().rev())
        .collect()
}

/// `Staged - Unstaged`, with `shown` in the accent.
fn column_tabs(shown: Column, theme: &Theme) -> Vec<Span<'static>> {
    let active = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(Color::Reset);
    let tab = |column: Column, label: &'static str| {
        let style = if column == shown { active } else { inactive };
        Span::styled(label, style)
    };
    vec![
        tab(Column::Staged, "Staged"),
        Span::styled(" - ", inactive),
        tab(Column::Unstaged, "Unstaged"),
    ]
}

/// Cheap stand-in for a not-yet-highlighted file: any type-change note
/// plus a muted "Rendering…" line. No syntect, so holding
/// `j` across many files never blocks. Its height is fixed and known, so
/// the assembled window has a definite length every frame.
fn placeholder_file_block(file: &FileDiff, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("")];
    if let Some(note) = typechange_note(file, theme) {
        lines.push(Line::from(""));
        lines.extend(note);
    }
    lines.push(Line::from(Span::styled(
        "Rendering…".to_string(),
        Style::default().fg(rgb_to_color(theme.muted)),
    )));
    lines
}

/// What to show when the pane has no files. The clean/no-repo case is a
/// single centered "No local changes." line; `Loading` is the same
/// centered treatment for a startup that is still resolving its first
/// diff (a repo was found but the initial build lost a race); a build
/// error is the error message, painted top-aligned and wrapped so
/// multi-line text (a message plus a `hint:` line) is fully visible.
#[derive(PartialEq, Eq)]
struct WindowKey {
    files: Option<Vec<usize>>,
    header: Vec<String>,
    epoch: CacheEpoch,
    blocks: u64,
    comments: u64,
    labels: u64,
    hide_low: bool,
    view: WindowView,
}

/// What a window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowView {
    Diffs,
    /// The summary of the files in this directory (empty for a commit).
    Summary(String),
    /// One file's changed definitions.
    Definitions,
}

/// `revision` changes whenever `lookup` may answer differently. With
/// `hide_low`, hunks whose label is low value collapse to one muted row.
pub(super) struct HunkLabels<'a> {
    pub(super) revision: u64,
    pub(super) lookup: &'a dyn Fn(usize, usize) -> Option<RowNote>,
    pub(super) hide_low: bool,
}

#[cfg(test)]
impl HunkLabels<'_> {
    pub(super) const NONE: HunkLabels<'static> = HunkLabels {
        revision: 0,
        lookup: &|_, _| None,
        hide_low: false,
    };
}

enum EmptyPane {
    NoChanges,
    Loading,
    Error(String),
}

/// The diff pane's owned state. The retained per-file line cache plus the
/// bookkeeping needed to scroll it and keep it aligned with the sidebar's
/// selection.
pub(super) struct DiffPane {
    /// The staging column whose bodies this pane renders.
    column: Column,
    /// Retained per-file rendered blocks.
    pub(super) cache: DiffCache,
    /// The width the last window was assembled at.
    pub(super) cached_width: usize,
    /// What the border title names.
    title: PaneTitle,
    /// True when the pane's column has no files, so it draws its empty state.
    empty: bool,
    /// The diff cursor's row and the pane's scroll offset, both relative
    /// to the top of the current selection's assembled window.
    pub(super) cursor: Cursor,
    cursor_ready: bool,
    /// The last-assembled window. Retained so key handling between draws
    /// works against exactly the rows on screen.
    window: Vec<DiffRow>,
    window_key: Option<WindowKey>,
    file_starts: Vec<(usize, usize)>,
    /// Retained definitions blocks: what each file's change did to its
    /// definitions, for summaries.
    pub(super) definitions: DiffCache,
    /// Retained outline blocks: each file's definitions, the changed ones
    /// among the unchanged ones around them, for the definitions view.
    pub(super) outline: DiffCache,
    /// Set by a reload: the next render scrolls the restored cursor back
    /// into view. Ordinary scrolling deliberately leaves the cursor
    /// behind, so this is only armed when the window was rebuilt under
    /// the user.
    reveal_cursor: bool,
    /// Set by a reload whose top-of-viewport file survived: that file's new
    /// index and the scroll offset into it. The next assembly puts the
    /// viewport back there.
    reload_top: Option<(usize, usize)>,
    /// Set when a file is visited: the first render of its complete block
    /// scrolls to the first hunk and puts the cursor on its first change.
    /// Any scroll or cursor move before then cancels it.
    landing: bool,
    /// True when the window holds one file whose block is fully rendered.
    window_complete: bool,
    /// What the no-files render shows: the clean state or a build error.
    empty_state: EmptyPane,
    /// The commits whose `git show` headers top the window, in order.
    header: Vec<git::Commit>,
    /// Low-value hunks are collapsed; the title says so.
    hide_low: bool,
    /// The title naming the current file, set before each render; the
    /// pane's own title when `None`.
    heading: Option<Line<'static>>,
}

impl DiffPane {
    pub(super) fn new(column: Column, width: usize) -> Self {
        Self {
            column,
            cache: DiffCache::default(),
            cached_width: width,
            title: PaneTitle::Diff,
            empty: true,
            cursor: Cursor::default(),
            cursor_ready: true,
            window: Vec::new(),
            window_key: None,
            file_starts: Vec::new(),
            definitions: DiffCache::of(BlockKind::Definitions),
            outline: DiffCache::of(BlockKind::Outline),
            reveal_cursor: false,
            reload_top: None,
            landing: true,
            window_complete: false,
            empty_state: EmptyPane::NoChanges,
            header: Vec::new(),
            hide_low: false,
            heading: None,
        }
    }

    /// Title the next render with `heading` (the current file), or with
    /// the pane's own title for `None`.
    pub(super) fn set_heading(&mut self, heading: Option<Line<'static>>) {
        self.heading = heading;
    }

    /// Settle what a render of `area` will show: land on the first hunk
    /// of a newly visited file, bring a moved cursor back into view, and
    /// clamp the scroll. [`DiffPane::render`] does this too; calling it
    /// first lets the caller ask what will be on screen.
    pub(super) fn settle(&mut self, area: Rect) {
        if self.empty {
            return;
        }
        let viewport = diff_scrollbar::body_area(area).height as usize;
        if self.landing && self.window_complete {
            self.landing = false;
            if let Some((scroll, row)) = land(&self.window, viewport) {
                select_row(&self.window, row, &mut self.cursor);
                self.cursor_ready = true;
                self.cursor.scroll = scroll;
            }
        }
        if self.cursor_ready
            && self
                .window
                .get(self.cursor.row)
                .is_some_and(DiffRow::is_selectable)
            && std::mem::take(&mut self.reveal_cursor)
        {
            keep_visible(&mut self.cursor, viewport.max(1));
        }
        self.cursor.scroll = self.cursor.scroll.min(self.max_scroll(viewport));
    }

    /// The file the pane is on in `area`: the cursor's when its line is
    /// on screen, otherwise the one at the top of the viewport.
    pub(super) fn current_file(&self, model: &Model, area: Rect) -> Option<usize> {
        if self.empty {
            return None;
        }
        let height = diff_scrollbar::body_area(area).height as usize;
        let on_screen =
            self.cursor.row >= self.cursor.scroll && self.cursor.row < self.cursor.scroll + height;
        let cursor_file = self
            .cursor_anchor()
            .filter(|_| on_screen)
            .and_then(|anchor| model.index_of(&anchor.path));
        cursor_file.or_else(|| self.top_file().map(|(file, _)| file))
    }

    /// Top the window with each of `commits`' headers, scrolling with the
    /// diff, or drop the header with none. A header shows even when the
    /// selection has no files.
    pub(super) fn set_header(&mut self, commits: &[git::Commit]) {
        if self.header != commits {
            self.header = commits.to_vec();
            self.window_key = None;
            self.cursor.scroll = 0;
        }
    }

    /// The staging column this pane renders.
    pub(super) fn column(&self) -> Column {
        self.column
    }

    /// Switch the no-files render to show a build-error message instead of
    /// the clean "No local changes." state.
    pub(super) fn set_empty_error(&mut self, msg: String) {
        self.empty_state = EmptyPane::Error(msg);
    }

    /// Switch the no-files render to a neutral "Loading…" line, used while
    /// a repo-backed startup resolves its first diff.
    pub(super) fn set_empty_loading(&mut self) {
        self.empty_state = EmptyPane::Loading;
    }

    /// Whether the no-files render shows a build error.
    pub(super) fn shows_error(&self) -> bool {
        matches!(self.empty_state, EmptyPane::Error(_))
    }

    /// Reset the no-files render to the clean "No local changes." state
    /// (used once a startup that was Loading resolves to a stable tree).
    pub(super) fn clear_empty_state(&mut self) {
        self.empty_state = EmptyPane::NoChanges;
    }

    /// Record what this draw shows: title, emptiness, width, and
    /// whether low-value hunks are collapsed.
    fn adopt_spec(&mut self, spec: &PaneSpec<'_>, width: usize, hide_low: bool) {
        self.hide_low = hide_low;
        debug_assert_eq!(spec.column, self.column);
        self.cached_width = width;
        self.title = spec.title;
        self.empty = spec.order.is_empty() && self.header.is_empty();
    }

    /// Assemble the selected window's file blocks into one line vector and
    /// record its length in `window_rows`. Missing blocks render outside
    /// the UI thread and contribute placeholders until ready. Reuse the
    /// assembled window while its rows stay unchanged. `spec` names the
    /// pane's files in display order and the selection's slice of them: a
    /// single file, a directory subtree, or `None`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble_window(
        &mut self,
        spec: &PaneSpec<'_>,
        model: &Model,
        width: usize,
        theme: &Theme,
        _budget: DrawBudget,
        comments: &CommentStore,
        labels: &HunkLabels<'_>,
    ) {
        self.adopt_spec(spec, width, labels.hide_low);
        let display_range = spec.range.clone();
        let mut key = WindowKey {
            files: display_range
                .clone()
                .map(|range| spec.order[range].to_vec()),
            header: self.header.iter().map(|commit| commit.id.clone()).collect(),
            epoch: CacheEpoch {
                width,
                syntax_theme: deltoids::theme_name_key(&theme.syntax_theme_name),
            },
            blocks: self.cache.revision,
            comments: comments.revision(),
            labels: labels.revision,
            hide_low: labels.hide_low,
            view: WindowView::Diffs,
        };
        let same_selection = self
            .window_key
            .as_ref()
            .is_some_and(|old| old.files == key.files && old.view == WindowView::Diffs);
        let reload_top = self.reload_top.take();
        let old_top = if same_selection {
            self.top_file()
        } else {
            reload_top
        };
        self.cache.request(
            key.epoch,
            model,
            self.column,
            spec.order,
            display_range.clone(),
            old_top.map(|(file, _)| file),
            theme,
        );
        if same_selection
            && self
                .window_key
                .as_ref()
                .is_some_and(|old| old.epoch != key.epoch)
            && self.window.iter().any(DiffRow::is_selectable)
            && key.files.as_ref().is_some_and(|files| {
                files
                    .iter()
                    .any(|file| !self.cache.complete(key.epoch, *file))
            })
        {
            return;
        }
        if self.window_key.as_ref() == Some(&key) {
            return;
        }
        let files = key.files.clone().unwrap_or_default();
        self.window_complete = self.header.is_empty()
            && matches!(files.as_slice(), [file] if self.cache.complete(key.epoch, *file));
        if files.is_empty() {
            self.set_header_window(key, width, theme);
            return;
        }

        let (window, starts) = self.build_window(&files, model, key.epoch, theme, comments, labels);
        if let Some((file, offset)) = old_top {
            match starts.iter().find(|(index, _)| *index == file) {
                Some((_, start)) => self.cursor.scroll = start + offset,
                None if reload_top.is_some() => self.reset_after_reload(),
                None => {}
            }
        }
        self.reveal_cursor |= same_selection
            && self
                .window_key
                .as_ref()
                .is_some_and(|old| old.epoch != key.epoch);
        self.file_starts = starts;
        let cursor_present = self
            .cursor
            .place()
            .is_none_or(|place| window.iter().any(|row| row.place.as_ref() == Some(place)));
        let cursor_ready = cursor_present
            || self
                .cursor
                .file()
                .and_then(|path| {
                    model
                        .files
                        .iter()
                        .position(|file| display_path(&file.file) == path)
                })
                .is_none_or(|index| self.cache.complete(key.epoch, index));
        self.set_window(window, cursor_ready);
        key.blocks = self.cache.revision;
        self.window_key = Some(key);
    }

    /// Assemble what `selection`'s change did to its definitions in place
    /// of diffs: a summary (totals, then each file's header and block,
    /// under the commit header for a whole commit), or for one file, its
    /// block alone. Blocks come from the definitions cache, drawn in the
    /// background.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble_definitions(
        &mut self,
        spec: &PaneSpec<'_>,
        model: &Model,
        selection: &overview::Selection<'_>,
        summary: bool,
        width: usize,
        theme: &Theme,
        labels: &HunkLabels<'_>,
    ) {
        self.adopt_spec(spec, width, labels.hide_low);
        let epoch = CacheEpoch {
            width,
            syntax_theme: deltoids::theme_name_key(&theme.syntax_theme_name),
        };
        self.cache
            .request(epoch, model, self.column, spec.order, None, None, theme);
        let (wanted, idle) = if summary {
            (&mut self.definitions, &mut self.outline)
        } else {
            (&mut self.outline, &mut self.definitions)
        };
        idle.request(epoch, model, self.column, spec.order, None, None, theme);
        wanted.request(
            epoch,
            model,
            self.column,
            spec.order,
            spec.range.clone(),
            None,
            theme,
        );
        let cache = if summary {
            &self.definitions
        } else {
            &self.outline
        };
        let key = WindowKey {
            files: Some(selection.files.to_vec()),
            header: self.header.iter().map(|commit| commit.id.clone()).collect(),
            epoch,
            blocks: cache.revision,
            comments: selection.comments.revision(),
            labels: labels.revision,
            hide_low: labels.hide_low,
            view: if summary {
                WindowView::Summary(selection.directory.to_string())
            } else {
                WindowView::Definitions
            },
        };
        if self.window_key.as_ref() == Some(&key) {
            return;
        }
        let blocks: Vec<Option<&[DiffRow]>> = selection
            .files
            .iter()
            .map(|&file| cache.get(epoch, file))
            .collect();
        let rows = if summary {
            let mut rows = self.header_rows(width, theme);
            rows.extend(overview::rows(model, selection, &blocks, width, theme));
            rows
        } else {
            blocks
                .first()
                .copied()
                .flatten()
                .map_or_else(|| vec![overview::placeholder(theme)], <[DiffRow]>::to_vec)
        };
        self.window_complete = false;
        self.file_starts.clear();
        self.set_window(rows, true);
        self.window_key = Some(key);
    }

    /// Where opening window row `row` goes, for a summary or definitions
    /// row.
    pub(super) fn opens_at(&self, row: usize) -> Option<&Opens> {
        self.window.get(row)?.opens.as_ref()
    }

    /// Where opening the cursor's row goes.
    pub(super) fn cursor_opens(&self) -> Option<&Opens> {
        self.cursor_ready
            .then(|| self.opens_at(self.cursor.row))
            .flatten()
    }

    /// Adopt finished background renders; whether any shows on screen.
    pub(super) fn collect(&mut self) -> bool {
        let diffs = self.cache.collect();
        let definitions = self.definitions.collect();
        let outline = self.outline.collect();
        diffs || definitions || outline
    }

    /// Whether a background render is still running.
    pub(super) fn pending(&self) -> bool {
        self.cache.pending() || self.definitions.pending() || self.outline.pending()
    }

    /// Stop background renders, keeping what is drawn.
    pub(super) fn pause(&mut self) {
        self.cache.pause();
        self.definitions.pause();
        self.outline.pause();
    }

    /// The window's rows: the commit header, then `files`' blocks, each
    /// after a divider naming it when the window holds more than one file
    /// (or a commit header). Also where each file starts.
    fn build_window(
        &self,
        files: &[usize],
        model: &Model,
        epoch: CacheEpoch,
        theme: &Theme,
        comments: &CommentStore,
        labels: &HunkLabels<'_>,
    ) -> (Vec<DiffRow>, Vec<(usize, usize)>) {
        let mut window = self.header_rows(epoch.width, theme);
        let dividers = files.len() > 1 || !self.header.is_empty();
        let mut starts = Vec::with_capacity(files.len());
        for (i, &index) in files.iter().enumerate() {
            if i > 0 {
                window.push(DiffRow::plain(Line::from("")));
            }
            starts.push((index, window.len()));
            if dividers {
                window.push(DiffRow::plain(file_divider(
                    model,
                    index,
                    epoch.width,
                    theme,
                )));
            }
            window.extend(self.file_block(index, model, epoch, theme, comments, labels));
        }
        (window, starts)
    }

    /// The border with its title, flagged while low-value hunks are
    /// collapsed.
    fn draw_border(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        color: Color,
        footer: Option<Line<'static>>,
        theme: &Theme,
    ) {
        let title = match &self.heading {
            Some(heading) if !self.empty => heading.clone(),
            _ => heading(self.title, theme),
        };
        let block = pane_block_with_title_line(title, color, footer);
        frame.render_widget(block, area);
        if self.hide_low {
            draw_filter_flag(frame, area, theme);
        }
    }

    /// A window of no files: the header's rows alone, kept until `key`
    /// changes, or nothing without a header.
    fn set_header_window(&mut self, key: WindowKey, width: usize, theme: &Theme) {
        let header = self.header_rows(width, theme);
        let keep = !header.is_empty();
        self.file_starts.clear();
        self.set_window(header, false);
        if keep {
            self.window_key = Some(key);
        }
    }

    /// The commit header's rows, which the cursor skips; none without one.
    fn header_rows(&self, width: usize, theme: &Theme) -> Vec<DiffRow> {
        self.header
            .iter()
            .flat_map(|commit| commit_header(commit, width, theme))
            .map(DiffRow::plain)
            .collect()
    }

    /// Adopt a freshly-assembled window and put the cursor back on the
    /// diff line it was on, falling back to the nearest line to its old
    /// row when that line is gone.
    fn set_window(&mut self, window: Vec<DiffRow>, cursor_ready: bool) {
        self.window = window;
        self.cursor_ready = cursor_ready;
        if cursor_ready && self.window.iter().any(DiffRow::is_selectable) {
            restore_cursor(&self.window, &mut self.cursor);
        }
    }

    /// Row count of the last-assembled window.
    pub(super) fn window_rows(&self) -> usize {
        self.window.len()
    }

    /// Drop the assembled window (a reload invalidates every row).
    pub(super) fn reset_window(&mut self) {
        self.window.clear();
        self.file_starts.clear();
        self.window_key = None;
    }

    /// The rows currently on screen.
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[DiffRow] {
        &self.window
    }

    /// The diff line under the cursor, when it sits on one.
    pub(super) fn cursor_anchor(&self) -> Option<&CommentAnchor> {
        if !self.cursor_ready {
            return None;
        }
        self.window.get(self.cursor.row)?.anchor.as_ref()
    }

    /// Re-render file `index` after its body changed shape, keeping the
    /// cursor on the file line `anchor` at its screen row.
    pub(super) fn reshape_file(&mut self, index: usize, anchor: CommentAnchor) {
        self.landing = false;
        self.cache.refresh(index);
        self.cursor.retarget(anchor);
        self.window_key = None;
    }

    /// Put the cursor on `anchor`'s line, a third of the way down the
    /// viewport, as soon as rows that draw it exist: a jump to a
    /// definition or a caller. Cancels landing on the first hunk.
    pub(super) fn jump_to(&mut self, anchor: CommentAnchor, viewport: usize) {
        self.landing = false;
        self.reload_top = None;
        self.cursor.jump(anchor, viewport / 3);
        self.cursor_ready = true;
        self.window_key = None;
    }

    /// The body rect inside `area` and the row, within it, where the
    /// cursor's line starts, when that line is on screen. Valid after
    /// [`DiffPane::render`] into `area`.
    pub(super) fn cursor_on_screen(&self, area: Rect) -> Option<(Rect, usize)> {
        if self.empty || !self.cursor_ready {
            return None;
        }
        self.window
            .get(self.cursor.row)
            .filter(|row| row.is_selectable())?;
        let body = diff_scrollbar::body_area(area);
        let offset = self.cursor.row.checked_sub(self.cursor.scroll)?;
        (offset < body.height as usize).then_some((body, offset))
    }

    /// The diff line drawn on window row `row`: the row its first row
    /// sits on, the file line it draws, and which wrapped row of it
    /// `row` is.
    pub(super) fn line_at(&self, row: usize) -> Option<(usize, &CommentAnchor, usize)> {
        let anchor = self.window.get(row)?.anchor.as_ref()?;
        let first = (0..=row).rev().find(|&index| {
            let candidate = &self.window[index];
            candidate.place.is_some() && candidate.anchor.as_ref() == Some(anchor)
        })?;
        Some((first, anchor, row - first))
    }

    /// Where the cursor sits: its rendered line and the file line it
    /// draws, when it is on a diff line.
    pub(super) fn cursor_line(&self) -> Option<(&LinePlace, &CommentAnchor)> {
        Some((self.cursor.place()?, self.cursor_anchor()?))
    }

    /// One file's block for the current frame: the retained highlighted
    /// lines, with comments and hunk labels, when ready; otherwise a cheap
    /// placeholder.
    fn file_block(
        &self,
        input_idx: usize,
        model: &Model,
        epoch: CacheEpoch,
        theme: &Theme,
        comments: &CommentStore,
        labels: &HunkLabels<'_>,
    ) -> Vec<DiffRow> {
        if let Some(rows) = self.cache.get(epoch, input_idx) {
            let mut rows = with_comments(rows, comments, epoch.width, theme, |anchor, comment| {
                model
                    .line(anchor)
                    .is_some_and(|(content, _)| content != comment.code)
            });
            let label = |hunk| (labels.lookup)(input_idx, hunk);
            label_hunks(&mut rows, epoch.width, theme, label);
            if labels.hide_low {
                rows = hide_low_hunks(rows, theme, label);
            }
            rows
        } else {
            let view = model.view(input_idx, self.column);
            placeholder_file_block(view.file, theme)
                .into_iter()
                .map(DiffRow::plain)
                .collect()
        }
    }

    /// Maximum scroll offset (relative to the window top) that keeps the
    /// viewport inside the assembled window.
    fn max_scroll(&self, viewport: usize) -> usize {
        self.window_rows().saturating_sub(viewport.max(1))
    }

    pub(super) fn scroll_by(&mut self, delta: isize, viewport: usize) {
        self.landing = false;
        let max = self.max_scroll(viewport) as isize;
        let target = (self.cursor.scroll as isize + delta).clamp(0, max.max(0));
        self.cursor.scroll = target as usize;
    }

    /// Move the diff cursor one diff line, keeping it in view.
    pub(super) fn step_cursor(&mut self, step: Step, viewport: usize) {
        self.landing = false;
        step_cursor(&self.window, step, viewport, &mut self.cursor);
        self.cursor_ready = true;
    }

    /// Put the cursor on `row` when that row starts a diff line. Used by
    /// a click in the pane.
    pub(super) fn select_row(&mut self, row: usize) {
        self.landing = false;
        select_row(&self.window, row, &mut self.cursor);
        self.cursor_ready = true;
    }

    fn scroll_to_top(&mut self) {
        self.landing = false;
        self.cursor.scroll = 0;
    }

    fn scroll_to_bottom(&mut self, viewport: usize) {
        self.landing = false;
        self.cursor.scroll = self.max_scroll(viewport);
    }

    /// Reset the view to the top of the current selection's window. A
    /// sidebar move re-derives the window, so the newly selected file
    /// starts from scroll 0 with the cursor on the window's first diff
    /// line, until its block is rendered and the view lands on its first
    /// hunk (see [`land`]).
    pub(super) fn snap_to_top(&mut self) {
        self.cursor = Cursor::default();
        self.window_key = None;
        self.reveal_cursor = false;
        self.reload_top = None;
        self.landing = true;
    }

    /// Adopt a rebuilt model. `survivors` maps old to new indices of the
    /// files whose view renders the same. When the file at the top of the
    /// viewport survived, the next assembly keeps the viewport where it
    /// was. Otherwise the view starts from the top of the window, with the
    /// cursor still on its diff line and revealed again. A working tree
    /// changes under the reviewer; their place in it should not.
    pub(super) fn carry_over(&mut self, survivors: &HashMap<usize, usize>) {
        let top = self.reload_top.take().or_else(|| self.top_file());
        self.reload_top = top.and_then(|(file, offset)| Some((*survivors.get(&file)?, offset)));
        if self.reload_top.is_none() {
            self.reset_after_reload();
        }
        self.cache.carry_over(survivors);
        self.definitions.carry_over(survivors);
        self.outline.carry_over(survivors);
        self.reset_window();
    }

    fn reset_after_reload(&mut self) {
        self.cursor.scroll = 0;
        self.reveal_cursor = true;
    }

    /// The file at the top of the viewport and the scroll offset into it.
    fn top_file(&self) -> Option<(usize, usize)> {
        self.file_starts
            .iter()
            .rev()
            .find(|(_, start)| *start <= self.cursor.scroll)
            .map(|(file, start)| (*file, self.cursor.scroll - start))
    }

    /// Handle a key while the diff pane is focused: `j`/`k` walk diff
    /// lines, or scroll a row when there are none (a header alone); the
    /// rest scroll. Everything else is ignored.
    pub(super) fn handle_key(&mut self, key: KeyCode, viewport: usize) {
        let lines = self.window.iter().any(DiffRow::is_selectable);
        match key {
            KeyCode::Char('j') | KeyCode::Down if !lines => self.scroll_by(1, viewport),
            KeyCode::Char('k') | KeyCode::Up if !lines => self.scroll_by(-1, viewport),
            KeyCode::Char('j') | KeyCode::Down => {
                self.step_cursor(Step::Down, viewport);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.step_cursor(Step::Up, viewport);
            }
            KeyCode::PageDown => {
                self.scroll_by(viewport.max(1) as isize, viewport);
            }
            KeyCode::PageUp => {
                self.scroll_by(-(viewport.max(1) as isize), viewport);
            }
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to_bottom(viewport),
            _ => {}
        }
    }

    /// Display the window last assembled by
    /// [`DiffPane::assemble_window`]: clamp the scroll, slice the
    /// viewport, highlight the cursor, and paint the footer + scrollbar.
    pub(super) fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        status: Option<&str>,
    ) {
        let color = pane_border_color(focused, theme);

        // With no files, render an empty state rather than a blank pane:
        // either the clean "No local changes." line (a reverted/committed
        // tree or a non-repo) or a build-error message.
        if self.empty {
            let footer = status.map(|message| Line::from(format!(" {message} ")));
            self.draw_border(frame, area, color, footer, theme);
            let inner = area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            match &self.empty_state {
                EmptyPane::NoChanges | EmptyPane::Loading => {
                    let text = match self.empty_state {
                        EmptyPane::Loading => "Loading\u{2026}",
                        _ => "No local changes.",
                    };
                    let msg = Paragraph::new(text)
                        .style(Style::default().fg(rgb_to_color(theme.muted)))
                        .alignment(Alignment::Center);
                    let mid = inner.height / 2;
                    let line = Rect {
                        x: inner.x,
                        y: inner.y.saturating_add(mid),
                        width: inner.width,
                        height: 1.min(inner.height),
                    };
                    frame.render_widget(msg, line);
                }
                // A build error can be multi-line (message + `hint:`), so
                // paint it top-aligned and wrapped, not one centered line.
                EmptyPane::Error(text) => {
                    let msg = Paragraph::new(text.clone())
                        .style(Style::default().fg(rgb_to_color(theme.muted)))
                        .wrap(Wrap { trim: false });
                    frame.render_widget(msg, inner);
                }
            }
            return;
        }

        let inner = diff_scrollbar::body_area(area);
        let viewport = inner.height as usize;
        self.settle(area);
        let scroll = self.cursor.scroll;
        let end = scroll
            .saturating_add(viewport.max(1))
            .min(self.window.len());
        let cursor_bg = rgb_to_color(theme.selection_bg);
        let width = inner.width as usize;
        let visible: Vec<Line<'static>> = self.window[scroll..end]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                if focused
                    && self.cursor_ready
                    && scroll + offset == self.cursor.row
                    && row.is_selectable()
                {
                    highlight_row(row.line.clone(), width, cursor_bg)
                } else {
                    row.line.clone()
                }
            })
            .collect();

        let footer = status
            .map(|status| format!(" {status} "))
            .or_else(|| self.footer())
            .map(Line::from);
        self.draw_border(frame, area, color, footer, theme);
        frame.render_widget(Paragraph::new(visible), inner);

        // Vertical scrollbar reflects the assembled window: when the
        // sidebar is on a directory the scrollbar tracks progress through
        // that subtree's files.
        diff_scrollbar::render(frame, area, &self.window, scroll, focused, theme);
    }

    /// Build the diff pane's bottom-right footer: `" line X of Y "` for the
    /// current scroll position within the assembled window, or `None` when
    /// the pane is empty. Reads the assembled window, so it is meaningful
    /// only after [`DiffPane::assemble_window`].
    pub(super) fn footer(&self) -> Option<String> {
        let span = self.window_rows();
        if span == 0 {
            return None;
        }
        let pos = self.cursor.scroll.min(span.saturating_sub(1)) + 1;
        let progress = if self.cache.visible_pending()
            || self.definitions.visible_pending()
            || self.outline.visible_pending()
        {
            "  ·  Rendering…"
        } else {
            ""
        };
        Some(format!(
            " line {pos} of {span}{progress}  \u{00b7}  ? help "
        ))
    }
}

/// Where a visited file opens: the scroll offset and the cursor row.
///
/// The first hunk's box goes to the top of the viewport when the hunk
/// fits, or when its first change shows in the top two thirds that way.
/// Otherwise the first change sits a third of the way down, so a hunk
/// expanded to a long scope still opens on its change. `None` when the
/// window has no boxed hunk (binary, symlink, type change), which keeps
/// the view at the top. The caller clamps the scroll, so a file that
/// fits on screen keeps its header in view.
fn land(rows: &[DiffRow], viewport: usize) -> Option<(usize, usize)> {
    let top = rows.iter().position(|row| row.hunk_start)?;
    let change = top
        + rows[top..].iter().position(|row| {
            row.is_selectable() && matches!(row.kind, Some(LineKind::Added | LineKind::Removed))
        })?;
    let end = rows[top + 1..]
        .iter()
        .position(|row| row.hunk_start)
        .map_or(rows.len(), |next| top + next);
    let viewport = viewport.max(1);
    let scroll = if end - top <= viewport || change < top + viewport * 2 / 3 {
        top
    } else {
        change - viewport / 3
    };
    Some((scroll, change))
}

/// The pane's border title. With both columns in the selection it names
/// both, the shown one highlighted as in the sidebar's mode strip.
fn heading(title: PaneTitle, theme: &Theme) -> Line<'static> {
    let label = |column| match column {
        Column::Staged => "Staged",
        Column::Unstaged => "Unstaged",
    };
    match title {
        PaneTitle::Diff => Line::from("─Diff─"),
        PaneTitle::Uncommitted => Line::from("─Uncommitted changes─"),
        PaneTitle::One(column) => Line::from(format!("─{} changes─", label(column))),
        PaneTitle::Both(shown) => {
            let mut spans = vec![Span::raw("─")];
            spans.extend(column_tabs(shown, theme));
            spans.push(Span::raw("─"));
            Line::from(spans)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::model::ResolvedFile;
    use crate::cli::browse::files::test_support::*;

    fn cache_epoch(width: usize) -> CacheEpoch {
        CacheEpoch {
            width,
            // Match the theme the render path keys on (test_support `theme()`),
            // so a render-produced epoch compares equal to this helper's.
            syntax_theme: deltoids::theme_name_key(&theme().syntax_theme_name),
        }
    }
    use crate::cli::browse::files::{Focus, handle_key};

    fn commit(message: &str) -> git::Commit {
        git::Commit {
            id: "c4b48c0d58e6c879f3e5287150a98f32e4a81f76".into(),
            short_id: "c4b48c0".into(),
            summary: message.lines().next().unwrap_or_default().into(),
            message: message.into(),
            author: "Juan Ibiapina".into(),
            email: "juanibiapina@gmail.com".into(),
            time: 1_791_896_835,
        }
    }

    fn header_text(commit: &git::Commit, width: usize) -> Vec<String> {
        commit_header(commit, width, &Theme::default())
            .iter()
            .map(Line::to_string)
            .collect()
    }

    #[test]
    fn the_commit_header_reads_like_git_show() {
        let commit = commit("style(tui): fit the help row\n\nIt wrapped.\n");

        assert_eq!(
            header_text(&commit, 80),
            [
                "commit c4b48c0d58e6c879f3e5287150a98f32e4a81f76",
                "Author: Juan Ibiapina <juanibiapina@gmail.com>",
                format!("Date:   {}", git_date(1_791_896_835, &TimeZone::system())).as_str(),
                "",
                "    style(tui): fit the help row",
                "",
                "    It wrapped.",
                "",
            ]
        );
    }

    #[test]
    fn dates_read_like_git_in_the_given_zone() {
        let berlin = TimeZone::fixed(jiff::tz::offset(2));

        assert_eq!(
            git_date(1_791_896_835, &berlin),
            "Tue Oct 13 15:07:15 2026 +0200"
        );
        assert_eq!(git_date(0, &TimeZone::UTC), "Thu Jan 1 00:00:00 1970 +0000");
    }

    #[test]
    fn long_message_lines_wrap_under_the_indent() {
        let commit = commit("one two three four");

        assert_eq!(
            header_text(&commit, 14)[4..],
            ["    one two", "    three four", ""]
        );
    }

    /// Concatenate every cell symbol of a rendered `TestBackend` buffer.
    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn empty_model_state_has_empty_display_order() {
        // Guards the startup empty-state render path: a clean repo opens
        // the TUI with no files, and the diff render keys the "No local
        // changes." message off `display_order.is_empty()`.
        let state = make_state(&[]);
        assert!(
            state.display_order.is_empty(),
            "expected empty display order for a zero-file model"
        );
    }

    #[test]
    fn empty_error_state_renders_message_not_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut pane = DiffPane::new(Column::Unstaged, 80);
        pane.set_empty_error("missing index blob deadbeef\nhint: try again".to_string());

        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), None);
        })
        .unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(
            text.contains("missing index blob"),
            "expected error message in: {text:?}"
        );
        assert!(
            text.contains("hint: try again"),
            "expected the hint line in: {text:?}"
        );
        assert!(
            !text.contains("No local changes."),
            "error state must not show the clean message: {text:?}"
        );
    }

    #[test]
    fn empty_loading_state_renders_loading_not_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut pane = DiffPane::new(Column::Unstaged, 80);
        pane.set_empty_loading();

        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), None);
        })
        .unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(
            text.contains("Loading"),
            "loading state shows the loading line: {text:?}"
        );
        assert!(
            !text.contains("No local changes."),
            "loading state must not show the clean message: {text:?}"
        );
    }

    #[test]
    fn file_overview_marks_wrapped_changes_in_both_layouts_and_palettes() {
        use crate::cli::browse::mode::Mode;
        use deltoids::ColorMode;
        use ratatui::{Terminal, backend::TestBackend};

        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: format!("context\n{}\n", "old ".repeat(20)),
            after: format!("context\n{}\n", "new ".repeat(20)),
        }];
        let mut state = make_state(&resolved);
        for mode in [ColorMode::Dark, ColorMode::Light] {
            let theme = Theme::for_mode(mode);
            let mut terminal = Terminal::new(TestBackend::new(60, 40)).unwrap();
            assemble_unstaged(
                &mut state.unstaged,
                &state.model,
                &state.sidebar,
                27,
                &theme,
            );
            wait_for_render(&mut state.unstaged);
            terminal
                .draw(|frame| {
                    state.draw(
                        frame,
                        Rect::new(0, 0, 30, 40),
                        Rect::new(30, 0, 30, 40),
                        &theme,
                        DrawBudget::Full,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let added_count = (1..39)
                .filter(|&y| {
                    buffer[(58, y)].symbol() == "█"
                        && buffer[(58, y)].fg == rgb_to_color(theme.status_added)
                })
                .count();
            let removed_count = (1..39)
                .filter(|&y| {
                    buffer[(58, y)].symbol() == "█"
                        && buffer[(58, y)].fg == rgb_to_color(theme.status_deleted)
                })
                .count();
            assert!(added_count > 1);
            assert!(removed_count > 1);
            assert_eq!(state.unstaged.cached_width, 27);
            assert_eq!(buffer[(59, 0)].symbol(), "╮");
            assert_eq!(buffer[(59, 39)].symbol(), "╯");
        }
    }

    #[test]
    fn the_focused_pane_paints_the_cursor_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "one\ntwo\n".to_string(),
            after: "one\nTWO\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let _ = state.visible_diff_window(DrawBudget::Full);
        let draw = |focused: bool, pane: &mut DiffPane| {
            let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
            term.draw(|f| {
                let area = f.area();
                pane.render(f, area, focused, &theme(), None);
            })
            .unwrap();
            term
        };
        draw(false, &mut state.unstaged);
        let cursor_row = state.unstaged.cursor.row;

        let selection_bg = rgb_to_color(theme().selection_bg);
        let bg_of_row = |focused: bool, pane: &mut DiffPane| {
            let term = draw(focused, pane);
            // Row 0 of the body sits one row below the pane border.
            let y = (cursor_row + 1) as u16;
            term.backend().buffer()[(1, y)].style().bg
        };

        assert_eq!(
            bg_of_row(true, &mut state.unstaged),
            Some(selection_bg),
            "the focused pane highlights the line the cursor is on"
        );
        assert_ne!(
            bg_of_row(false, &mut state.unstaged),
            Some(selection_bg),
            "an unfocused pane draws no cursor"
        );
    }

    #[test]
    fn empty_default_state_renders_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut pane = DiffPane::new(Column::Unstaged, 80);
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), None);
        })
        .unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(
            text.contains("No local changes."),
            "default empty state shows the clean message: {text:?}"
        );
    }

    #[test]
    fn new_state_defers_highlighting_but_populates_sidebar() {
        // First paint: the diff cache is empty (highlighting deferred) but
        // the sidebar already has every file's row (counts present).
        let resolved = vec![resolved("a.txt"), resolved("b.txt")];
        let state = make_state(&resolved);
        assert!(
            state.unstaged.cache.is_empty(),
            "diff cache should start empty (highlighting deferred)"
        );
        assert_eq!(state.sidebar.display_order().len(), 2);
    }

    #[test]
    fn diff_cache_retains_blocks_across_selection_changes() {
        let mut cache = DiffCache::default();
        let e = cache_epoch(80);
        cache.insert(e, 0, vec![DiffRow::plain(Line::from("a"))]);
        cache.insert(
            e,
            1,
            vec![
                DiffRow::plain(Line::from("b")),
                DiffRow::plain(Line::from("b2")),
            ],
        );

        // Both files stay retained: revisiting the first is a cache hit.
        assert!(cache.contains(e, 0));
        assert!(cache.contains(e, 1));
        assert_eq!(cache.get(e, 0).map(|l| l.len()), Some(1));
        assert_eq!(cache.get(e, 1).map(|l| l.len()), Some(2));
    }

    #[test]
    fn diff_cache_width_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(cache_epoch(80), 0, vec![DiffRow::plain(Line::from("a"))]);
        assert!(cache.contains(cache_epoch(80), 0));

        // A different width drops the stale block and rebuilds at the new width.
        assert!(!cache.contains(cache_epoch(79), 0));
        assert!(cache.get(cache_epoch(79), 0).is_none());
        cache.insert(cache_epoch(79), 1, vec![DiffRow::plain(Line::from("b"))]);
        assert!(!cache.contains(cache_epoch(79), 0));
        assert!(cache.contains(cache_epoch(79), 1));
    }

    #[test]
    fn diff_cache_syntax_theme_change_clears_store() {
        let mut cache = DiffCache::default();
        let mono = CacheEpoch {
            width: 80,
            syntax_theme: "Monokai Extended",
        };
        let tokyo = CacheEpoch {
            syntax_theme: "TokyoNight",
            ..mono
        };
        cache.insert(mono, 0, vec![DiffRow::plain(Line::from("a"))]);
        assert!(cache.contains(mono, 0));
        // A live theme switch changes the epoch, so the store is dropped and
        // every block re-highlights under the new theme.
        assert!(!cache.contains(tokyo, 0));
    }

    #[test]
    fn a_single_file_window_shows_its_hunks_without_a_header() {
        let resolved = vec![ResolvedFile {
            file: file_diff("foo.txt"),
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let window = state.visible_diff_window(DrawBudget::Full);
        let texts: Vec<String> = window.iter().map(line_text).collect();

        assert!(
            !texts.iter().any(|t| t.contains("foo.txt")),
            "the name is on the border, not in the body: {texts:#?}"
        );
        assert!(texts.iter().any(|t| t.contains("hello")), "{texts:#?}");
        assert!(texts.iter().any(|t| t.contains("world")), "{texts:#?}");
        assert_eq!(window_files(&state), ["foo.txt"]);
    }

    #[test]
    fn assemble_window_renders_symlink_view_for_symlink_file() {
        use deltoids::parse::{FileDiff, RawHunk, RawLine, RawLineKind};

        let file = FileDiff {
            preamble: Vec::new(),
            old_path: "link.txt".to_string(),
            new_path: "link.txt".to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            old_mode: None,
            new_mode: Some("120000".to_string()),
            hunks: vec![RawHunk {
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: 1,
                lines: vec![RawLine {
                    kind: RawLineKind::Added,
                    content: "a.txt".to_string(),
                }],
            }],
        };
        let resolved = vec![ResolvedFile {
            file,
            before: String::new(),
            after: String::new(),
        }];
        let mut state = make_state(&resolved);
        let window = state.visible_diff_window(DrawBudget::Full);
        let texts: Vec<String> = window.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|t| t.contains("symlink created")),
            "expected the symlink view, got: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("\u{2192} a.txt")),
            "expected the symlink body, got: {texts:#?}"
        );
    }

    #[test]
    fn assemble_window_renders_typechange_note_for_type_change() {
        // A regular file → symlink type change renders as a content diff
        // plus a note clarifying the file became a symlink.
        let mut file = file_diff("f.txt");
        file.preamble = vec!["old mode 100644".to_string(), "new mode 120000".to_string()];
        let resolved = vec![ResolvedFile {
            file,
            before: "hello\nworld\n".to_string(),
            after: "target.txt\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let texts: Vec<String> = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("type change: regular file \u{2192} symlink")),
            "expected the type-change note, got: {texts:#?}"
        );
        // The content diff still renders alongside the note.
        assert!(
            texts.iter().any(|t| t.contains("hello")),
            "expected the removed content, got: {texts:#?}"
        );
        // Only the note box is drawn: no redundant per-hunk line-number
        // box (which would add a second box-top line ending in `╮`).
        let box_tops = texts.iter().filter(|t| t.ends_with('╮')).count();
        assert_eq!(box_tops, 1, "expected exactly one box, got: {texts:#?}");
    }

    #[test]
    fn assemble_window_renders_binary_placeholder() {
        let resolved = vec![binary_resolved("bin")];
        let mut state = make_state(&resolved);
        let window = state.visible_diff_window(DrawBudget::Full);
        let texts: Vec<String> = window.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|t| t.contains("Binary file")),
            "expected the binary placeholder, got: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.starts_with("@@")),
            "binary body must render no hunk lines, got: {texts:#?}"
        );
    }

    #[test]
    fn assemble_window_renders_submodule_placeholder() {
        let resolved = vec![submodule_resolved("sub", "399c80dabc", "099e72cdef")];
        let mut state = make_state(&resolved);
        let window = state.visible_diff_window(DrawBudget::Full);
        let texts: Vec<String> = window.iter().map(line_text).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Submodule") && t.contains("399c80d") && t.contains("099e72c")),
            "expected the submodule placeholder with short commits, got: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.starts_with("@@")),
            "submodule body must render no hunk lines, got: {texts:#?}"
        );
    }

    #[test]
    fn assemble_window_renders_typechange_note_and_submodule_placeholder() {
        let resolved = vec![submodule_typechange_resolved("sub", "099e72cdef")];
        let mut state = make_state(&resolved);
        let texts: Vec<String> = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("type change: regular file \u{2192} submodule")),
            "expected the type-change note, got: {texts:#?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Submodule") && t.contains("099e72c")),
            "expected the submodule placeholder below the note, got: {texts:#?}"
        );
    }

    #[test]
    fn completed_render_is_retained() {
        let resolved = vec![resolved("a.txt")];
        let mut state = make_state(&resolved);
        assert!(state.unstaged.cache.is_empty());

        // The test helper waits for completion before reading retained rows.
        let _ = state.visible_diff_window(DrawBudget::Full);
        assert!(state.unstaged.cache.contains(cache_epoch(80), 0));
    }

    #[test]
    fn fast_frame_starts_rendering_and_completion_is_usable_without_settling() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert!(state.unstaged.cache.is_empty());

        // An uncached file submits work and immediately shows a placeholder.
        let window = state.visible_diff_window(DrawBudget::Fast);
        let texts: Vec<String> = window.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|t| t.contains("Rendering")),
            "placeholder should show a Rendering line, got: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("hello")),
            "placeholder must not highlight the diff body, got: {texts:#?}"
        );
        assert!(
            state.unstaged.cache.is_empty(),
            "Fast frame must not cache the file"
        );

        wait_for_render(&mut state.unstaged);
        let ready = state.visible_diff_window(DrawBudget::Fast);
        assert!(ready.iter().any(|line| line_text(line).contains("world")));
        assert!(state.unstaged.cache.contains(cache_epoch(80), 0));
    }

    #[test]
    fn the_border_shows_the_files_syntax_support() {
        let border = |path: &str| -> String {
            let resolved = vec![ResolvedFile {
                file: file_diff(path),
                before: "val x = 1\n".to_string(),
                after: "val x = 2\n".to_string(),
            }];
            let mut state = make_state(&resolved);
            crate::cli::browse::test_screen::screen(&mut state).remove(0)
        };
        let kotlin = border("Main.kt");
        assert!(kotlin.contains("Kotlin ─╮"), "{kotlin}");
        let docker = border("Dockerfile");
        assert!(
            docker.contains("Dockerfile · no scope context ─╮"),
            "{docker}"
        );
        let plain = border("notes.txt");
        assert!(plain.contains("─notes.txt "), "{plain}");
        assert!(plain.ends_with("──╮"), "plain text has no badge: {plain}");
    }

    #[test]
    fn a_window_of_several_files_divides_them_in_display_order() {
        let resolved = vec![resolved("b.txt"), resolved("a.txt")];
        let mut state = make_state(&resolved);
        state.show_whole(true);
        let texts: Vec<String> = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect();
        let divider = |path: &str| {
            texts
                .iter()
                .position(|t| t.starts_with(&format!("── {path} ─")))
                .unwrap_or_else(|| panic!("no divider for {path}: {texts:#?}"))
        };
        assert!(divider("a.txt") < divider("b.txt"));
    }

    #[test]
    fn a_rename_is_named_old_to_new_on_the_border() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let border = crate::cli::browse::test_screen::screen(&mut state).remove(0);
        assert!(border.contains("─old.txt → new.txt "), "{border}");
    }

    #[test]
    fn handle_key_j_in_diff_focus_walks_diff_lines_and_follows_them() {
        // Build a diff with enough lines to scroll.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        // Prime the window so the cursor has rows to walk.
        let _ = state.visible_diff_window(DrawBudget::Full);
        state.focus = Focus::Diff;

        let first = state
            .unstaged
            .cursor_anchor()
            .cloned()
            .expect("a diff line");
        handle_key(&mut state, KeyCode::Char('j'), 4);
        let second = state
            .unstaged
            .cursor_anchor()
            .cloned()
            .expect("a diff line");
        assert_ne!(first, second, "j moves to the next diff line");
        // The pane scrolls only as much as it takes to keep it visible.
        assert!(state.unstaged.cursor.row >= state.unstaged.cursor.scroll);
        assert!(state.unstaged.cursor.row < state.unstaged.cursor.scroll + 4);
    }

    #[test]
    fn handle_key_capital_j_scrolls_diff_in_sidebar_focus() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        let _ = state.visible_diff_window(DrawBudget::Full);
        // Stay in Sidebar focus; Shift+J should still scroll the diff.
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Char('J'), 4);
        assert_eq!(state.unstaged.cursor.scroll, SCROLL_STEP_LARGE);
    }

    /// The files whose lines the unstaged window draws, in order.
    fn window_files(state: &super::super::FilesMode) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        for anchor in state
            .unstaged
            .rows()
            .iter()
            .filter_map(|row| row.anchor.as_ref())
        {
            if !files.contains(&anchor.path) {
                files.push(anchor.path.clone());
            }
        }
        files
    }

    #[test]
    fn diff_footer_includes_help_hint() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let _ = state.visible_diff_window(DrawBudget::Full);
        let footer = state.unstaged.footer().expect("footer present");
        assert!(
            footer.contains("? help"),
            "expected '? help' hint in footer, got {footer:?}"
        );
    }

    #[test]
    fn scroll_on_diff_scrolls_content() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state_with_rects(&resolved);
        let _ = state.visible_diff_window(DrawBudget::Full);
        state.focus = Focus::Diff;
        let before = state.unstaged.cursor.scroll;

        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollDown, 50, 5);
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18);
        assert!(state.unstaged.cursor.scroll > before);

        let after_down = state.unstaged.cursor.scroll;
        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 50, 5);
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18);
        assert!(state.unstaged.cursor.scroll < after_down);
    }

    #[test]
    fn shift_horizontal_wheel_steps_files_over_diff() {
        let resolved: Vec<_> = ["a.txt", "b.txt", "c.txt"]
            .into_iter()
            .map(|path| ResolvedFile {
                file: file_diff(path),
                before: "old\n".to_string(),
                after: "new\n".to_string(),
            })
            .collect();
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Diff;
        state.unstaged.cursor.scroll = 4;

        let right = make_mouse_mods(
            crossterm::event::MouseEventKind::ScrollRight,
            50,
            5,
            crossterm::event::KeyModifiers::SHIFT,
        );
        crate::cli::browse::files::handle_mouse(&mut state, right, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        assert_eq!(state.unstaged.cursor.scroll, 0);
        assert_eq!(state.focus, Focus::Diff);

        crate::cli::browse::files::handle_mouse(&mut state, right, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(2));
        crate::cli::browse::files::handle_mouse(&mut state, right, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(2));

        let left = make_mouse_mods(
            crossterm::event::MouseEventKind::ScrollLeft,
            50,
            5,
            crossterm::event::KeyModifiers::SHIFT,
        );
        crate::cli::browse::files::handle_mouse(&mut state, left, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        crate::cli::browse::files::handle_mouse(
            &mut state,
            make_mouse(crossterm::event::MouseEventKind::ScrollRight, 50, 5),
            18,
        );
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
    }

    #[test]
    fn ctrl_scroll_on_diff_moves_sidebar() {
        // Hovering the diff with Ctrl held redirects the wheel to the
        // sidebar list instead of scrolling the diff.
        let resolved = vec![
            ResolvedFile {
                file: file_diff("a.txt"),
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: file_diff("b.txt"),
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state_with_rects(&resolved);
        let _ = state.visible_diff_window(DrawBudget::Full);
        let initial = state.sidebar.selected();
        let diff_before = state.unstaged.cursor.scroll;

        // Cursor over the diff (col 50), Ctrl held.
        let mouse = make_mouse_mods(
            crossterm::event::MouseEventKind::ScrollDown,
            50,
            5,
            crossterm::event::KeyModifiers::CONTROL,
        );
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18);

        assert!(
            state.sidebar.selected() > initial,
            "ctrl+scroll should move the sidebar selection"
        );
        assert_eq!(
            state.unstaged.cursor.scroll, diff_before,
            "ctrl+scroll should not scroll the diff"
        );
    }
}
