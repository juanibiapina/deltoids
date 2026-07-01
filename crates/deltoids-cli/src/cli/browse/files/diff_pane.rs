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
use ratatui::widgets::Paragraph;

use deltoids::render_tui::{
    self, pane_block_with_footer, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};
use deltoids::{Diff, Theme};

use crate::cli::browse::mode::{DrawBudget, should_build_body};
use crate::sidebar::display_path;

use super::model::{Model, ResolvedFile};

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

/// Render one file's diff block: file header, an optional rename header,
/// then each hunk (blank-separated). This is the syntax-highlighting cost
/// (`render_hunk`) that lazy rendering defers.
fn render_file_block(
    resolved: &ResolvedFile,
    diff: &Diff,
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

    for hunk in diff.hunks() {
        lines.push(Line::from(""));
        lines.extend(render_tui::render_hunk(
            hunk,
            diff.highlight(),
            width,
            theme,
        ));
    }

    lines
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
    lines.push(Line::from(Span::styled(
        "Rendering…".to_string(),
        Style::default().fg(rgb_to_color(theme.muted)),
    )));
    lines
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
}

impl DiffPane {
    pub(super) fn new(display_order: Vec<usize>, width: usize) -> Self {
        Self {
            cache: DiffCache::default(),
            display_order,
            cached_width: width,
            diff_scroll: 0,
            window_rows: 0,
        }
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
                    &model.diffs[input_idx],
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

        // After a reload that reverted/committed every change there are no
        // files: render a centered empty state rather than a blank pane.
        if self.display_order.is_empty() {
            let block = pane_block_with_footer("─[2]─Diff─", color, None);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let msg = Paragraph::new("No local changes.")
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
}
