//! Diff pane vertical slice: its state (a retained per-file line cache,
//! display order, scroll), its scroll math, its key handling, and its
//! render. The pane is always filtered to whatever the sidebar points at;
//! the shell passes that selection range in as a plain value so this slice
//! never reaches into the sidebar's fields.
//!
//! Rendering is lazy and budget-aware, mirroring Traces mode: only the
//! files the current selection needs are highlighted, and highlighting is
//! deferred while navigation input streams (`DrawBudget::Fast`), filling in
//! and being retained once input settles (`Full`). The lazy unit is the
//! *file*, keyed by input file index in [`DiffCache`].

use std::collections::HashMap;
use std::ops::Range;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use deltoids::Theme;
use deltoids::render_tui::{
    self, pane_block_with_footer, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};

use deltoids::parse::FileDiff;

use crate::cli::browse::mode::{DrawBudget, should_build_body};
use crate::sidebar::{FileMode, IconMode, ModeChange, display_path, file_metadata, symlink_icon};

use super::model::{FileBody, Model, ResolvedFile};

pub(super) const SCROLL_STEP_SMALL: usize = 1;
pub(super) const SCROLL_STEP_LARGE: usize = 3;

/// Retained store of rendered per-file diff blocks, keyed by *input* file
/// index. Every retained block shares one `width`; a width change clears
/// the store (mirroring Traces mode's `DiffCache`). Retaining rendered
/// files makes revisiting a file instant instead of re-highlighting it on
/// every selection change.
#[derive(Debug, Default)]
pub(super) struct DiffCache {
    width: usize,
    lines: HashMap<usize, Vec<Line<'static>>>,
}

impl DiffCache {
    /// Rendered lines for file `key` at `width`, or `None` on a width
    /// mismatch or a miss.
    fn get(&self, width: usize, key: usize) -> Option<&Vec<Line<'static>>> {
        if self.width != width {
            return None;
        }
        self.lines.get(&key)
    }

    /// Whether file `key` is already rendered at `width`.
    fn contains(&self, width: usize, key: usize) -> bool {
        self.width == width && self.lines.contains_key(&key)
    }

    /// Store rendered `lines` for file `key`. A width change clears the
    /// store first so every retained block shares one width.
    fn insert(&mut self, width: usize, key: usize, lines: Vec<Line<'static>>) {
        if self.width != width {
            self.lines.clear();
            self.width = width;
        }
        self.lines.insert(key, lines);
    }

    /// Drop all retained blocks (used on reload, when disk data changed).
    pub(super) fn clear(&mut self) {
        self.lines.clear();
    }

    /// Whether the store holds no retained blocks.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Render one file's block: file header, an optional rename header, then
/// the body — either each hunk (blank-separated) for a text diff, or the
/// symlink view for a symlink change. The text-diff path carries the
/// syntax-highlighting cost (`render_hunk`) that lazy rendering defers.
fn render_file_block(
    resolved: &ResolvedFile,
    body: &FileBody,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let path = display_path(&resolved.file);
    lines.extend(render_tui::render_file_header(path, width, theme));

    if let Some(old_path) = &resolved.file.rename_from {
        lines.push(render_tui::render_rename_header(
            old_path,
            &resolved.file.new_path,
            theme,
        ));
    }
    // A type change renders as a content diff, but its note box stands in
    // for the per-hunk line-number box: render the note box, then the hunk
    // bodies without their own boxes (avoiding a second, redundant box).
    if let Some(note) = typechange_note(&resolved.file, theme) {
        lines.push(Line::from(""));
        lines.extend(note);
        match body {
            FileBody::Diff(diff) => {
                for hunk in diff.hunks() {
                    lines.push(Line::from(""));
                    lines.extend(render_tui::render_hunk_body(
                        hunk,
                        diff.highlight(),
                        width,
                        theme,
                    ));
                }
            }
            // A type change *into* a submodule (regular → submodule) has no
            // textual body; render the placeholder below the note box so the
            // pane is not empty.
            FileBody::Submodule {
                old_commit,
                new_commit,
            } => {
                lines.push(Line::from(""));
                lines.push(submodule_placeholder(
                    old_commit.as_deref(),
                    new_commit.as_deref(),
                    theme,
                ));
            }
            _ => {}
        }
        return lines;
    }

    match body {
        FileBody::Diff(diff) => lines.extend(render_tui::render_hunk_list(
            diff.hunks(),
            diff.highlight(),
            width,
            theme,
        )),
        FileBody::Symlink(view) => {
            lines.push(Line::from(""));
            lines.extend(render_tui::render_symlink(
                view,
                symlink_icon(IconMode::from_env()),
                theme,
            ));
        }
        FileBody::Binary => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Binary file (no textual diff)".to_string(),
                Style::default().fg(rgb_to_color(theme.muted)),
            )));
        }
        FileBody::Submodule {
            old_commit,
            new_commit,
        } => {
            lines.push(Line::from(""));
            lines.push(submodule_placeholder(
                old_commit.as_deref(),
                new_commit.as_deref(),
                theme,
            ));
        }
    }

    lines
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

/// Cheap stand-in for a not-yet-highlighted file: the file header (and any
/// rename header) plus a muted "Rendering…" line. No syntect, so holding
/// `j` across many files never blocks. Its height is fixed and known, so
/// the assembled window has a definite length every frame.
fn placeholder_file_block(
    resolved: &ResolvedFile,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let path = display_path(&resolved.file);
    let mut lines = render_tui::render_file_header(path, width, theme);
    if let Some(old_path) = &resolved.file.rename_from {
        lines.push(render_tui::render_rename_header(
            old_path,
            &resolved.file.new_path,
            theme,
        ));
    }
    if let Some(note) = typechange_note(&resolved.file, theme) {
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
enum EmptyPane {
    NoChanges,
    Loading,
    Error(String),
}

/// The diff pane's owned state. The retained per-file line cache plus the
/// bookkeeping needed to scroll it and keep it aligned with the sidebar's
/// selection.
pub(super) struct DiffPane {
    /// Retained per-file rendered blocks.
    pub(super) cache: DiffCache,
    /// File indices in sidebar (display) order.
    pub(super) display_order: Vec<usize>,
    /// The width the cache was built for; a change clears it.
    pub(super) cached_width: usize,
    /// Vertical scroll offset, relative to the top of the current
    /// selection's assembled window.
    pub(super) diff_scroll: usize,
    /// Row count of the last-assembled visible window; drives scroll
    /// clamping between draws (mirrors Traces reading rows from its cache).
    pub(super) window_rows: usize,
    /// What the no-files render shows: the clean state or a build error.
    empty_state: EmptyPane,
}

impl DiffPane {
    pub(super) fn new(display_order: Vec<usize>, width: usize) -> Self {
        Self {
            cache: DiffCache::default(),
            display_order,
            cached_width: width,
            diff_scroll: 0,
            window_rows: 0,
            empty_state: EmptyPane::NoChanges,
        }
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

    /// Assemble the selected window's file blocks into one line vector and
    /// record its length in `window_rows`. Files the window needs are
    /// highlighted and retained on a `Full` frame; on a `Fast` frame an
    /// uncached file contributes a cheap placeholder block instead (and is
    /// not cached). `display_range` is the sidebar's selection range in
    /// display order: a single file, a directory subtree, or `None`.
    pub(super) fn assemble_window(
        &mut self,
        display_range: Option<Range<usize>>,
        model: &Model,
        width: usize,
        theme: &Theme,
        budget: DrawBudget,
    ) -> Vec<Line<'static>> {
        let Some(range) = display_range else {
            self.window_rows = 0;
            return Vec::new();
        };
        if range.is_empty() || self.display_order.is_empty() {
            self.window_rows = 0;
            return Vec::new();
        }

        let mut window = Vec::new();
        for (i, pos) in range.clone().enumerate() {
            let input_idx = self.display_order[pos];
            if i > 0 {
                window.push(Line::from(""));
            }
            window.extend(self.file_block(input_idx, model, width, theme, budget));
        }
        self.window_rows = window.len();
        window
    }

    /// One file's block for the current frame: the retained highlighted
    /// lines when cached; a fresh highlight (retained) when the budget
    /// allows building; otherwise a cheap placeholder.
    fn file_block(
        &mut self,
        input_idx: usize,
        model: &Model,
        width: usize,
        theme: &Theme,
        budget: DrawBudget,
    ) -> Vec<Line<'static>> {
        let cached = self.cache.contains(width, input_idx);
        if should_build_body(budget, cached) {
            if !cached {
                let lines = render_file_block(
                    &model.files[input_idx],
                    &model.bodies[input_idx],
                    width,
                    theme,
                );
                self.cache.insert(width, input_idx, lines);
            }
            self.cache
                .get(width, input_idx)
                .cloned()
                .unwrap_or_default()
        } else {
            placeholder_file_block(&model.files[input_idx], width, theme)
        }
    }

    /// Maximum scroll offset (relative to the window top) that keeps the
    /// viewport inside the assembled window.
    fn max_scroll(&self, viewport: usize) -> usize {
        self.window_rows.saturating_sub(viewport.max(1))
    }

    pub(super) fn scroll_by(&mut self, delta: isize, viewport: usize) {
        let max = self.max_scroll(viewport) as isize;
        let target = (self.diff_scroll as isize + delta).clamp(0, max.max(0));
        self.diff_scroll = target as usize;
    }

    fn scroll_to_top(&mut self) {
        self.diff_scroll = 0;
    }

    fn scroll_to_bottom(&mut self, viewport: usize) {
        self.diff_scroll = self.max_scroll(viewport);
    }

    /// Reset the scroll to the top of the current selection's window. A
    /// sidebar move re-derives the window, so showing the newly selected
    /// file from its top is always scroll 0.
    pub(super) fn snap_to_top(&mut self) {
        self.diff_scroll = 0;
    }

    /// Handle a key while the diff pane is focused. Only scroll keys are
    /// meaningful; everything else is ignored.
    pub(super) fn handle_key(&mut self, key: KeyCode, viewport: usize) {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_by(SCROLL_STEP_SMALL as isize, viewport);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_by(-(SCROLL_STEP_SMALL as isize), viewport);
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

    /// Display the pre-assembled `window` (built by
    /// [`DiffPane::assemble_window`], which also set `window_rows`): clamp
    /// the scroll, slice the viewport, and paint the footer + scrollbar.
    pub(super) fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        window: Vec<Line<'static>>,
    ) {
        let color = pane_border_color(focused, theme);

        // With no files, render an empty state rather than a blank pane:
        // either the clean "No local changes." line (a reverted/committed
        // tree or a non-repo) or a build-error message.
        if self.display_order.is_empty() {
            let block = pane_block_with_footer("─[2]─Diff─", color, None);
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

        let inner = area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let viewport = inner.height as usize;

        let scroll = self.diff_scroll.min(self.max_scroll(viewport));
        self.diff_scroll = scroll;
        let end = scroll.saturating_add(viewport.max(1)).min(window.len());
        let visible: Vec<Line<'static>> = window[scroll..end].to_vec();

        let footer = self.footer();
        let block = pane_block_with_footer("─[2]─Diff─", color, footer);
        frame.render_widget(Paragraph::new(visible).block(block), area);

        // Vertical scrollbar reflects the assembled window: when the
        // sidebar is on a directory the scrollbar tracks progress through
        // that subtree's files.
        render_pane_scrollbar(
            frame,
            area,
            self.window_rows,
            scroll,
            pane_inner_height(area),
            focused,
            theme,
        );
    }

    /// Build the diff pane's bottom-right footer: `" line X of Y "` for the
    /// current scroll position within the assembled window, or `None` when
    /// the pane is empty. Reads `window_rows`, so it is meaningful only
    /// after [`DiffPane::assemble_window`] (which `render` calls first).
    pub(super) fn footer(&self) -> Option<String> {
        let span = self.window_rows;
        if span == 0 {
            return None;
        }
        let pos = self.diff_scroll.min(span.saturating_sub(1)) + 1;
        Some(format!(" line {pos} of {span}  \u{00b7}  ? help "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::test_support::*;
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
            state.diff.display_order.is_empty(),
            "expected empty display order for a zero-file model"
        );
    }

    #[test]
    fn empty_error_state_renders_message_not_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut pane = DiffPane::new(Vec::new(), 80);
        pane.set_empty_error("missing index blob deadbeef\nhint: try again".to_string());

        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), Vec::new());
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

        let mut pane = DiffPane::new(Vec::new(), 80);
        pane.set_empty_loading();

        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), Vec::new());
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
    fn empty_default_state_renders_no_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut pane = DiffPane::new(Vec::new(), 80);
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            let area = f.area();
            pane.render(f, area, false, &theme(), Vec::new());
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
            state.diff.cache.is_empty(),
            "diff cache should start empty (highlighting deferred)"
        );
        assert_eq!(state.sidebar.display_order().len(), 2);
    }

    #[test]
    fn diff_cache_retains_blocks_across_selection_changes() {
        let mut cache = DiffCache::default();
        cache.insert(80, 0, vec![Line::from("a")]);
        cache.insert(80, 1, vec![Line::from("b"), Line::from("b2")]);

        // Both files stay retained: revisiting the first is a cache hit.
        assert!(cache.contains(80, 0));
        assert!(cache.contains(80, 1));
        assert_eq!(cache.get(80, 0).map(|l| l.len()), Some(1));
        assert_eq!(cache.get(80, 1).map(|l| l.len()), Some(2));
    }

    #[test]
    fn diff_cache_width_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(80, 0, vec![Line::from("a")]);
        assert!(cache.contains(80, 0));

        // A different width drops the stale block and rebuilds at the new width.
        assert!(!cache.contains(79, 0));
        assert!(cache.get(79, 0).is_none());
        cache.insert(79, 1, vec![Line::from("b")]);
        assert!(!cache.contains(79, 0));
        assert!(cache.contains(79, 1));
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
    fn assemble_window_full_frame_builds_and_retains() {
        let resolved = vec![resolved("a.txt")];
        let mut state = make_state(&resolved);
        assert!(state.diff.cache.is_empty());

        // A Full frame highlights the selected file and retains it.
        let _ = state.visible_diff_window(DrawBudget::Full);
        assert!(state.diff.cache.contains(80, 0));
    }

    #[test]
    fn assemble_window_fast_frame_shows_placeholder_without_caching() {
        let resolved = vec![ResolvedFile {
            file: file_diff("a.txt"),
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert!(state.diff.cache.is_empty());

        // A Fast frame on an uncached file shows the header placeholder and
        // does not populate the cache.
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
            state.diff.cache.is_empty(),
            "Fast frame must not cache the file"
        );

        // The settling Full frame renders and retains it.
        let _ = state.visible_diff_window(DrawBudget::Full);
        assert!(state.diff.cache.contains(80, 0));
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
    fn handle_key_j_in_diff_focus_scrolls_diff() {
        // Build a diff with enough lines to scroll.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        // Prime the window row count so scrolling has room.
        let _ = state.visible_diff_window(DrawBudget::Full);
        state.focus = Focus::Diff;
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
        assert_eq!(state.diff.diff_scroll, 1);
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
        assert_eq!(state.diff.diff_scroll, SCROLL_STEP_LARGE);
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
        // Two files in different dirs: src/a.rs and other/b.rs. Selecting a
        // dir restricts the window to that dir's file; selecting the other
        // dir restricts to the other file.
        let resolved = vec![
            ResolvedFile {
                file: file_diff("src/a.rs"),
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: file_diff("other/b.rs"),
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
            file_first == "src/a.rs" || file_first == "other/b.rs",
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
            first_line == "src/a.rs" || first_line == "other/b.rs",
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
        let footer = state.diff.footer().expect("footer present");
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
        let before = state.diff.diff_scroll;

        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollDown, 50, 5);
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff.diff_scroll > before);

        let after_down = state.diff.diff_scroll;
        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 50, 5);
        crate::cli::browse::files::handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff.diff_scroll < after_down);
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
        let diff_before = state.diff.diff_scroll;

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
            state.diff.diff_scroll, diff_before,
            "ctrl+scroll should not scroll the diff"
        );
    }
}
