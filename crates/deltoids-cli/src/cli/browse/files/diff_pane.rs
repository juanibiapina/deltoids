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

use deltoids::render_tui::{self, pane_block_with_tabs, pane_border_color, rgb_to_color};
use deltoids::{ChangeLayout, Hunk, Theme};

use deltoids::parse::FileDiff;

use crate::cli::browse::comment_view::{highlight_row, with_comments};
use crate::cli::browse::comments::{
    CommentAnchor, CommentScope, CommentStore, Numbering, numbered_lines,
};
use crate::cli::browse::diff_cursor::{
    Cursor, DiffRow, LinePlace, Step, keep_visible, restore_cursor, select_row, step_cursor,
};
use crate::cli::browse::diff_scrollbar;
use crate::cli::browse::mode::{DrawBudget, layout_label};
use crate::cli::browse::syntax_badge::with_syntax_badge;
use crate::sidebar::{FileMode, IconMode, ModeChange, display_path, file_metadata, symlink_icon};

use super::model::{Column, FileBody, Model};
use super::render::DiffCache;
use super::stage_panes::{PaneSpec, PaneTitle};

pub(super) const SCROLL_STEP_SMALL: usize = 1;
pub(super) const SCROLL_STEP_LARGE: usize = 3;

/// The identity every retained block shares: its render `width`, the
/// active change `layout`, and the selected `syntax_theme` (a `&'static`
/// registry name). A change to any of them invalidates the whole store,
/// since each alters every block's rendered rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheEpoch {
    pub(super) width: usize,
    pub(super) layout: ChangeLayout,
    pub(super) syntax_theme: &'static str,
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
    layout: ChangeLayout,
    theme: &Theme,
    keep_going: &impl Fn() -> bool,
    publish: &mut dyn FnMut(Vec<DiffRow>),
) -> Option<Vec<DiffRow>> {
    let mut render = FileRender {
        width,
        numbering,
        layout,
        theme,
        keep_going,
        preview: FilePreview {
            publish,
            published: false,
        },
    };
    let path = display_path(file);
    let mut rows: Vec<DiffRow> = file_header(file, body, width, theme)
        .into_iter()
        .map(DiffRow::plain)
        .collect();

    if let Some(old_path) = &file.rename_from {
        rows.push(DiffRow::plain(render_tui::render_rename_header(
            old_path,
            &file.new_path,
            theme,
        )));
    }
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
    layout: ChangeLayout,
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
                    self.layout,
                    self.theme,
                    &keep_going,
                    callback,
                )
            } else {
                render_tui::render_hunk_body_rows_with_preview(
                    hunk,
                    diff.highlight(),
                    self.width,
                    self.layout,
                    self.theme,
                    &keep_going,
                    callback,
                )
            }?;
            push_hunk_rows(rows, rendered, hunk, (path, index), self.numbering);
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
            scope: CommentScope::WorkingTree,
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

/// The file header, with the syntax badge on the path line for a text diff.
fn file_header(
    file: &FileDiff,
    body: &FileBody,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let path = display_path(file);
    let mut lines = render_tui::render_file_header(path, width, theme);
    if let FileBody::Diff(diff) = body {
        lines[0] = with_syntax_badge(
            lines[0].clone(),
            diff.language(),
            diff.highlight(),
            path,
            width,
            theme,
        );
    }
    lines
}

/// Cheap stand-in for a not-yet-highlighted file: the file header (and any
/// rename header) plus a muted "Rendering…" line. No syntect, so holding
/// `j` across many files never blocks. Its height is fixed and known, so
/// the assembled window has a definite length every frame.
fn placeholder_file_block(
    file: &FileDiff,
    body: &FileBody,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = file_header(file, body, width, theme);
    if let Some(old_path) = &file.rename_from {
        lines.push(render_tui::render_rename_header(
            old_path,
            &file.new_path,
            theme,
        ));
    }
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
    epoch: CacheEpoch,
    blocks: u64,
    comments: u64,
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
    /// Set by a reload: the next render scrolls the restored cursor back
    /// into view. Ordinary scrolling deliberately leaves the cursor
    /// behind, so this is only armed when the window was rebuilt under
    /// the user.
    reveal_cursor: bool,
    /// Set by a reload whose top-of-viewport file survived: that file's new
    /// index and the scroll offset into it. The next assembly puts the
    /// viewport back there.
    reload_top: Option<(usize, usize)>,
    /// The layout the last window was assembled with; shown in the footer.
    current_layout: ChangeLayout,
    /// What the no-files render shows: the clean state or a build error.
    empty_state: EmptyPane,
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
            reveal_cursor: false,
            reload_top: None,
            current_layout: ChangeLayout::Grouped,
            empty_state: EmptyPane::NoChanges,
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

    /// Reset the no-files render to the clean "No local changes." state
    /// (used once a startup that was Loading resolves to a stable tree).
    pub(super) fn clear_empty_state(&mut self) {
        self.empty_state = EmptyPane::NoChanges;
    }

    /// Record what this draw shows: title, emptiness, width, and layout.
    fn adopt_spec(&mut self, spec: &PaneSpec<'_>, width: usize, layout: ChangeLayout) {
        debug_assert_eq!(spec.column, self.column);
        self.current_layout = layout;
        self.cached_width = width;
        self.title = spec.title;
        self.empty = spec.order.is_empty();
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
        layout: ChangeLayout,
        theme: &Theme,
        _budget: DrawBudget,
        comments: &CommentStore,
    ) {
        self.adopt_spec(spec, width, layout);
        let display_range = spec.range.clone();
        let mut key = WindowKey {
            files: display_range
                .clone()
                .map(|range| spec.order[range].to_vec()),
            epoch: CacheEpoch {
                width,
                layout,
                syntax_theme: deltoids::theme_name_key(&theme.syntax_theme_name),
            },
            blocks: self.cache.revision,
            comments: comments.revision(),
        };
        let same_selection = self
            .window_key
            .as_ref()
            .is_some_and(|old| old.files == key.files);
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
        let Some(files) = key.files.clone() else {
            self.set_window(Vec::new(), false);
            return;
        };
        if files.is_empty() {
            self.set_window(Vec::new(), false);
            return;
        }

        let mut window = Vec::new();
        let mut starts = Vec::new();
        for (i, input_idx) in files.into_iter().enumerate() {
            if i > 0 {
                window.push(DiffRow::plain(Line::from("")));
            }
            starts.push((input_idx, window.len()));
            window.extend(self.file_block(input_idx, model, key.epoch, theme, comments));
        }
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
        self.cache.refresh(index);
        self.cursor.retarget(anchor);
        self.window_key = None;
    }

    /// Where the cursor sits: its rendered line and the file line it
    /// draws, when it is on a diff line.
    pub(super) fn cursor_line(&self) -> Option<(&LinePlace, &CommentAnchor)> {
        Some((self.cursor.place()?, self.cursor_anchor()?))
    }

    /// One file's block for the current frame: the retained highlighted
    /// lines when ready; otherwise a cheap placeholder.
    fn file_block(
        &self,
        input_idx: usize,
        model: &Model,
        epoch: CacheEpoch,
        theme: &Theme,
        comments: &CommentStore,
    ) -> Vec<DiffRow> {
        if let Some(rows) = self.cache.get(epoch, input_idx) {
            with_comments(rows, comments, epoch.width, theme, |anchor, comment| {
                model
                    .line(anchor)
                    .is_some_and(|(content, _)| content != comment.code)
            })
        } else {
            let view = model.view(input_idx, self.column);
            placeholder_file_block(view.file, view.body, epoch.width, theme)
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
        let max = self.max_scroll(viewport) as isize;
        let target = (self.cursor.scroll as isize + delta).clamp(0, max.max(0));
        self.cursor.scroll = target as usize;
    }

    /// Move the diff cursor one diff line, keeping it in view.
    pub(super) fn step_cursor(&mut self, step: Step, viewport: usize) {
        step_cursor(&self.window, step, viewport, &mut self.cursor);
        self.cursor_ready = true;
    }

    /// Put the cursor on `row` when that row starts a diff line. Used by
    /// a click in the pane.
    pub(super) fn select_row(&mut self, row: usize) {
        select_row(&self.window, row, &mut self.cursor);
        self.cursor_ready = true;
    }

    fn scroll_to_top(&mut self) {
        self.cursor.scroll = 0;
    }

    fn scroll_to_bottom(&mut self, viewport: usize) {
        self.cursor.scroll = self.max_scroll(viewport);
    }

    /// Reset the view to the top of the current selection's window. A
    /// sidebar move re-derives the window, so showing the newly selected
    /// file from its top is always scroll 0, with the cursor back on the
    /// window's first diff line.
    pub(super) fn snap_to_top(&mut self) {
        self.cursor = Cursor::default();
        self.window_key = None;
        self.reveal_cursor = false;
        self.reload_top = None;
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
    /// lines, the rest scroll. Everything else is ignored.
    pub(super) fn handle_key(&mut self, key: KeyCode, viewport: usize) {
        match key {
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
            let block = pane_block_with_tabs(
                heading(self.title, theme),
                color,
                status.map(|message| format!(" {message} ")),
            );
            let inner = block.inner(area);
            frame.render_widget(block, area);
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

        if self.cursor_ready
            && self
                .window
                .get(self.cursor.row)
                .is_some_and(DiffRow::is_selectable)
            && std::mem::take(&mut self.reveal_cursor)
        {
            keep_visible(&mut self.cursor, viewport.max(1));
        }
        let scroll = self.cursor.scroll.min(self.max_scroll(viewport));
        self.cursor.scroll = scroll;
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
            .or_else(|| self.footer());
        let block = pane_block_with_tabs(heading(self.title, theme), color, footer);
        frame.render_widget(block, area);
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
        let layout = layout_label(self.current_layout);
        let progress = if self.cache.visible_pending() {
            "  ·  Rendering…"
        } else {
            ""
        };
        Some(format!(
            " line {pos} of {span}{progress}  \u{00b7}  {layout}  \u{00b7}  ? help "
        ))
    }
}

/// The pane's border title. With both columns in the selection it names
/// both, the shown one highlighted as in the sidebar's mode strip.
fn heading(title: PaneTitle, theme: &Theme) -> Line<'static> {
    let label = |column| match column {
        Column::Staged => "Staged",
        Column::Unstaged => "Unstaged",
    };
    match title {
        PaneTitle::Diff => Line::from("─[2]─Diff─"),
        PaneTitle::One(column) => Line::from(format!("─[2]─{} changes─", label(column))),
        PaneTitle::Both(shown) => {
            let active = Style::default()
                .fg(rgb_to_color(theme.border_active))
                .add_modifier(Modifier::BOLD);
            let inactive = Style::default().fg(Color::Reset);
            let tab = |column| {
                let style = if column == shown { active } else { inactive };
                Span::styled(label(column), style)
            };
            Line::from(vec![
                Span::raw("─[2]─"),
                tab(Column::Staged),
                Span::styled(" - ", inactive),
                tab(Column::Unstaged),
                Span::raw("─"),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::model::ResolvedFile;
    use crate::cli::browse::files::test_support::*;

    fn grouped_epoch(width: usize) -> CacheEpoch {
        CacheEpoch {
            width,
            layout: ChangeLayout::Grouped,
            // Match the theme the render path keys on (test_support `theme()`),
            // so a render-produced epoch compares equal to this helper's.
            syntax_theme: deltoids::theme_name_key(&theme().syntax_theme_name),
        }
    }
    use crate::cli::browse::files::{Focus, handle_key};

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
        use crate::cli::browse::mode::{Mode, TabStrip};
        use deltoids::ColorMode;
        use ratatui::{Terminal, backend::TestBackend};

        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: format!("context\n{}\n", "old ".repeat(20)),
            after: format!("context\n{}\n", "new ".repeat(20)),
        }];
        let mut state = make_state(&resolved);
        let interleaved = ChangeLayout::Interleaved {
            group: std::num::NonZeroUsize::new(1).unwrap(),
        };
        for (mode, layout) in [
            (ColorMode::Dark, ChangeLayout::Grouped),
            (ColorMode::Light, ChangeLayout::Grouped),
            (ColorMode::Dark, interleaved),
            (ColorMode::Light, interleaved),
        ] {
            let theme = Theme::for_mode(mode);
            let mut terminal = Terminal::new(TestBackend::new(60, 40)).unwrap();
            assemble_unstaged(
                &mut state.unstaged,
                &state.model,
                &state.sidebar,
                27,
                layout,
                &theme,
            );
            wait_for_render(&mut state.unstaged);
            terminal
                .draw(|frame| {
                    state.draw(
                        frame,
                        Rect::new(0, 0, 30, 40),
                        Rect::new(30, 0, 30, 40),
                        TabStrip { active: 0 },
                        layout,
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
        let cursor_row = state.unstaged.cursor.row;

        let selection_bg = rgb_to_color(theme().selection_bg);
        let bg_of_row = |focused: bool, pane: &mut DiffPane| {
            let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
            term.draw(|f| {
                let area = f.area();
                pane.render(f, area, focused, &theme(), None);
            })
            .unwrap();
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
        let e = grouped_epoch(80);
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
        cache.insert(grouped_epoch(80), 0, vec![DiffRow::plain(Line::from("a"))]);
        assert!(cache.contains(grouped_epoch(80), 0));

        // A different width drops the stale block and rebuilds at the new width.
        assert!(!cache.contains(grouped_epoch(79), 0));
        assert!(cache.get(grouped_epoch(79), 0).is_none());
        cache.insert(grouped_epoch(79), 1, vec![DiffRow::plain(Line::from("b"))]);
        assert!(!cache.contains(grouped_epoch(79), 0));
        assert!(cache.contains(grouped_epoch(79), 1));
    }

    #[test]
    fn diff_cache_layout_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(grouped_epoch(80), 0, vec![DiffRow::plain(Line::from("a"))]);
        assert!(cache.contains(grouped_epoch(80), 0));

        // Same width, different layout: the stale block is dropped and
        // rebuilt under the new layout epoch.
        let interleaved = CacheEpoch {
            width: 80,
            layout: ChangeLayout::Interleaved {
                group: std::num::NonZeroUsize::new(1).unwrap(),
            },
            syntax_theme: "",
        };
        assert!(!cache.contains(interleaved, 0));
        cache.insert(interleaved, 1, vec![DiffRow::plain(Line::from("b"))]);
        assert!(!cache.contains(interleaved, 0));
        assert!(cache.contains(interleaved, 1));
    }

    #[test]
    fn diff_cache_syntax_theme_change_clears_store() {
        let mut cache = DiffCache::default();
        let mono = CacheEpoch {
            width: 80,
            layout: ChangeLayout::Grouped,
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
    fn assemble_window_emits_header_and_hunk_for_one_file() {
        let resolved = vec![ResolvedFile {
            file: file_diff("foo.txt"),
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let window = state.visible_diff_window(DrawBudget::Full);
        let texts: Vec<String> = window.iter().map(line_text).collect();

        assert!(
            texts.iter().any(|t| t == "foo.txt"),
            "expected file header, got: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("hello")),
            "expected removed line in: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("world")),
            "expected added line in: {texts:#?}"
        );
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
            texts.iter().any(|t| t == "bin"),
            "expected the file header, got: {texts:#?}"
        );
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
            texts.iter().any(|t| t == "sub"),
            "expected the file header, got: {texts:#?}"
        );
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
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 0));
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

        // An uncached file submits work and immediately shows its header.
        let window = state.visible_diff_window(DrawBudget::Fast);
        let texts: Vec<String> = window.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|t| t == "a.txt"),
            "placeholder should show the file header, got: {texts:#?}"
        );
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
        assert!(state.unstaged.cache.contains(grouped_epoch(80), 0));
    }

    #[test]
    fn file_headers_show_syntax_support_before_and_after_rendering() {
        let headers = |path: &str| -> (String, String) {
            let resolved = vec![ResolvedFile {
                file: file_diff(path),
                before: "val x = 1\n".to_string(),
                after: "val x = 2\n".to_string(),
            }];
            let mut state = make_state(&resolved);
            let placeholder = line_text(&state.visible_diff_window(DrawBudget::Fast)[0]);
            wait_for_render(&mut state.unstaged);
            let rendered = line_text(&state.visible_diff_window(DrawBudget::Fast)[0]);
            (placeholder, rendered)
        };

        for path in ["Main.kt", "Dockerfile", "notes.txt"] {
            let (placeholder, rendered) = headers(path);
            assert_eq!(placeholder, rendered, "{path}");
        }
        assert!(headers("Main.kt").1.ends_with(" Kotlin"));
        assert!(
            headers("Dockerfile")
                .1
                .ends_with(" Dockerfile · no scope context")
        );
        assert_eq!(headers("notes.txt").1, "notes.txt");
    }

    #[test]
    fn assemble_window_renders_in_display_order() {
        // Files supplied in input order [a, b]; the window walks display
        // order, so the first header is whichever file sorts first.
        let resolved = vec![resolved("b.txt"), resolved("a.txt")];
        let mut state = make_state(&resolved);
        // Select the directory-less root subtree by selecting nothing
        // special: instead, assert single-file selection shows its file.
        let window = state.visible_diff_window(DrawBudget::Full);
        let first = line_text(&window[0]);
        assert!(
            first == "a.txt" || first == "b.txt",
            "expected a file header first, got {first:?}"
        );
    }

    #[test]
    fn assemble_window_includes_rename_header_when_renamed() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        let combined: String = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("renamed:")
                && combined.contains("old.txt")
                && combined.contains("new.txt"),
            "missing rename header in: {combined}"
        );
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
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
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
        handle_key(&mut state, KeyCode::Char('J'), 4, 4);
        assert_eq!(state.unstaged.cursor.scroll, SCROLL_STEP_LARGE);
    }

    #[test]
    fn dir_filter_excludes_files_outside_subtree() {
        // Three files under three different dirs. Each file's diff has a
        // unique marker line so we can assert exactly which files are
        // visible at any time.
        let resolved = vec![
            ResolvedFile {
                file: file_diff("alpha/a.rs"),
                before: "old_alpha\n".to_string(),
                after: "new_alpha\n".to_string(),
            },
            ResolvedFile {
                file: file_diff("beta/b.rs"),
                before: "old_beta\n".to_string(),
                after: "new_beta\n".to_string(),
            },
            ResolvedFile {
                file: file_diff("gamma/c.rs"),
                before: "old_gamma\n".to_string(),
                after: "new_gamma\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        // Walk to the `beta/` dir header. Tree order: alpha/ (dir 0),
        // alpha/a.rs (file 0), beta/ (dir 1), beta/b.rs (file 1),
        // gamma/ (dir 2), gamma/c.rs (file 2). Initial selection is on
        // file 0 (alpha/a.rs at row 1). Step down to row 2 = beta/.
        state.sidebar.move_down(20);
        assert!(state.sidebar.selected_is_dir());

        let visible_text: String = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        // Only beta/b.rs's content must be inside the window.
        assert!(
            visible_text.contains("beta/b.rs")
                && visible_text.contains("old_beta")
                && visible_text.contains("new_beta"),
            "beta content missing from filtered window: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("alpha/a.rs") && !visible_text.contains("old_alpha"),
            "alpha leaked into beta filter: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("gamma/c.rs") && !visible_text.contains("old_gamma"),
            "gamma leaked into beta filter: {visible_text:?}"
        );

        // Move to a file row; window narrows to that single file.
        state.sidebar.move_down(20); // file row inside beta/
        assert!(!state.sidebar.selected_is_dir());
        let file_text: String = state
            .visible_diff_window(DrawBudget::Full)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            file_text.contains("beta/b.rs") && file_text.contains("new_beta"),
            "beta file content missing from filtered window: {file_text:?}"
        );
        assert!(
            !file_text.contains("alpha/a.rs") && !file_text.contains("gamma/c.rs"),
            "siblings leaked into single-file filter: {file_text:?}"
        );
    }

    #[test]
    fn window_narrows_to_subtree_on_dir_selection() {
        // Two files in different dirs: src/a.txt and other/b.txt. Selecting a
        // dir restricts the window to that dir's file; selecting the other
        // dir restricts to the other file.
        let resolved = vec![
            ResolvedFile {
                file: file_diff("src/a.txt"),
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: file_diff("other/b.txt"),
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);

        // Initial selection is on a file: window is exactly that file.
        let file_first = {
            let window = state.visible_diff_window(DrawBudget::Full);
            line_text(&window[0])
        };
        assert!(
            file_first == "src/a.txt" || file_first == "other/b.txt",
            "expected a file header at start, got {file_first:?}"
        );

        // Move up onto the dir header above the first file.
        state.sidebar.top(20);
        assert!(state.sidebar.selected_is_dir());
        let first_line = {
            let window = state.visible_diff_window(DrawBudget::Full);
            line_text(&window[0])
        };
        assert!(
            first_line == "src/a.txt" || first_line == "other/b.txt",
            "expected the dir's file header at start, got {first_line:?}"
        );
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
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.unstaged.cursor.scroll > before);

        let after_down = state.unstaged.cursor.scroll;
        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 50, 5);
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18, 18);
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
        crate::cli::browse::files::handle_mouse(&mut state, right, 18, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        assert_eq!(state.unstaged.cursor.scroll, 0);
        assert_eq!(state.focus, Focus::Diff);

        crate::cli::browse::files::handle_mouse(&mut state, right, 18, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(2));
        crate::cli::browse::files::handle_mouse(&mut state, right, 18, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(2));

        let left = make_mouse_mods(
            crossterm::event::MouseEventKind::ScrollLeft,
            50,
            5,
            crossterm::event::KeyModifiers::SHIFT,
        );
        crate::cli::browse::files::handle_mouse(&mut state, left, 18, 18);
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        crate::cli::browse::files::handle_mouse(
            &mut state,
            make_mouse(crossterm::event::MouseEventKind::ScrollRight, 50, 5),
            18,
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
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18, 18);

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
