//! Diff pane vertical slice: its state (the cached line stream, per-file
//! offsets, display order, scroll), its scroll/visible-range math, its
//! key handling, and its render. The pane is always filtered to whatever
//! the sidebar points at; the shell passes that selection range in as a
//! plain value so this slice never reaches into the sidebar's fields.

use std::ops::Range;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use deltoids::render_tui::{
    self, pane_block_with_footer, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};
use deltoids::{Diff, Theme};

use crate::sidebar::display_path;

use super::model::ResolvedFile;

pub(super) const SCROLL_STEP_SMALL: usize = 1;
pub(super) const SCROLL_STEP_LARGE: usize = 3;

/// Result of laying out all files into a single scrollable line stream.
pub(super) struct DiffView {
    pub(super) lines: Vec<Line<'static>>,
    /// `file_offsets[i]` is the row in `lines` where file `i`'s header
    /// starts. Used by the sidebar to scroll the diff pane in sync with
    /// file selection.
    pub(super) file_offsets: Vec<usize>,
}

/// Build the diff pane as a flat list of ratatui lines. Same layout as
/// before; renders files in `display_order` (sidebar tree order) so the
/// diff pane's vertical layout matches the sidebar exactly. The
/// returned `file_offsets` is keyed by *input* index — the caller looks
/// up `file_offsets[input_index]` to find where that file's header
/// starts in the rendered output.
pub(super) fn build_view(
    files: &[ResolvedFile],
    diffs: &[Diff],
    display_order: &[usize],
    width: usize,
    theme: &Theme,
) -> DiffView {
    let mut lines = Vec::new();
    let mut file_offsets = vec![0usize; files.len()];

    for (display_idx, &input_idx) in display_order.iter().enumerate() {
        if display_idx > 0 {
            lines.push(Line::from(""));
        }
        file_offsets[input_idx] = lines.len();

        let resolved = &files[input_idx];
        let path = display_path(&resolved.file);
        lines.extend(render_tui::render_file_header(path, width, theme));

        if let Some(old_path) = &resolved.file.rename_from {
            lines.push(render_tui::render_rename_header(
                old_path,
                &resolved.file.new_path,
                theme,
            ));
        }

        let diff = &diffs[input_idx];
        for hunk in diff.hunks() {
            lines.push(Line::from(""));
            lines.extend(render_tui::render_hunk(
                hunk,
                diff.highlight(),
                width,
                theme,
            ));
        }
    }

    DiffView {
        lines,
        file_offsets,
    }
}

/// The diff pane's owned state. The diff line cache plus the bookkeeping
/// needed to scroll it and keep it aligned with the sidebar's selection.
pub(super) struct DiffPane {
    /// Cached diff lines, valid for `cached_width`.
    pub(super) diff_lines: Vec<Line<'static>>,
    /// Per-file row offsets into `diff_lines`. Indexed by *input* index;
    /// the value is the line in `diff_lines` where that file starts.
    pub(super) file_offsets: Vec<usize>,
    /// File indices in sidebar (display) order. Cached so resize
    /// rebuilds reuse the same order.
    pub(super) display_order: Vec<usize>,
    /// The width `diff_lines` was built for; rebuild when the diff pane
    /// resizes.
    pub(super) cached_width: usize,
    /// Vertical scroll offset (in lines) for the diff pane.
    pub(super) diff_scroll: usize,
}

impl DiffPane {
    pub(super) fn new(view: DiffView, display_order: Vec<usize>, width: usize) -> Self {
        Self {
            diff_lines: view.lines,
            file_offsets: view.file_offsets,
            display_order,
            cached_width: width,
            diff_scroll: 0,
        }
    }

    /// Window of `diff_lines` that should be visible right now.
    ///
    /// The diff pane is always filtered to whatever the sidebar is
    /// pointing at: a directory header narrows to that subtree's
    /// files, a file row narrows to that single file. `display_range`
    /// is the sidebar's selection range (in display order), queried by
    /// the shell. Empty diff (no files at all) or no selection falls
    /// through to the full slice so the pane simply renders nothing.
    pub(super) fn visible_range(&self, display_range: Option<Range<usize>>) -> Range<usize> {
        let Some(display_range) = display_range else {
            return 0..self.diff_lines.len();
        };
        if display_range.is_empty() || self.display_order.is_empty() {
            return 0..self.diff_lines.len();
        }
        let first_input = self.display_order[display_range.start];
        let start = self.file_offsets[first_input];
        let end = if display_range.end < self.display_order.len() {
            // Stop just before the blank separator that precedes the
            // next file. file_offsets points at the file *header*, so
            // the line immediately above is the separator.
            let next_input = self.display_order[display_range.end];
            self.file_offsets[next_input].saturating_sub(1)
        } else {
            self.diff_lines.len()
        };
        start..end
    }

    /// Maximum scroll offset (an absolute index in `diff_lines`) such
    /// that the viewport still sits inside the current visible range.
    pub(super) fn max_scroll(&self, display_range: Option<Range<usize>>, viewport: usize) -> usize {
        let range = self.visible_range(display_range);
        let span = range.end.saturating_sub(range.start);
        range.start + span.saturating_sub(viewport.max(1))
    }

    /// Lower bound for `diff_scroll` (start of the visible range).
    pub(super) fn min_scroll(&self, display_range: Option<Range<usize>>) -> usize {
        self.visible_range(display_range).start
    }

    pub(super) fn scroll_by(
        &mut self,
        delta: isize,
        viewport: usize,
        display_range: Option<Range<usize>>,
    ) {
        let min = self.min_scroll(display_range.clone()) as isize;
        let max = self.max_scroll(display_range, viewport) as isize;
        let target = (self.diff_scroll as isize + delta).clamp(min, max.max(min));
        self.diff_scroll = target as usize;
    }

    fn scroll_to_top(&mut self, display_range: Option<Range<usize>>) {
        self.diff_scroll = self.min_scroll(display_range);
    }

    fn scroll_to_bottom(&mut self, viewport: usize, display_range: Option<Range<usize>>) {
        self.diff_scroll = self.max_scroll(display_range, viewport);
    }

    /// Sync the scroll to `file_idx` (the file the sidebar is pointing
    /// at), clamped to the current visible range.
    pub(super) fn snap_to_file(
        &mut self,
        file_idx: usize,
        viewport: usize,
        display_range: Option<Range<usize>>,
    ) {
        let Some(&offset) = self.file_offsets.get(file_idx) else {
            return;
        };
        let min = self.min_scroll(display_range.clone());
        let max = self.max_scroll(display_range, viewport);
        self.diff_scroll = offset.clamp(min, max.max(min));
    }

    /// Handle a key while the diff pane is focused. Only scroll keys are
    /// meaningful; everything else is ignored.
    pub(super) fn handle_key(
        &mut self,
        key: KeyCode,
        viewport: usize,
        display_range: Option<Range<usize>>,
    ) {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_by(SCROLL_STEP_SMALL as isize, viewport, display_range);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_by(-(SCROLL_STEP_SMALL as isize), viewport, display_range);
            }
            KeyCode::PageDown => {
                self.scroll_by(viewport.max(1) as isize, viewport, display_range);
            }
            KeyCode::PageUp => {
                self.scroll_by(-(viewport.max(1) as isize), viewport, display_range);
            }
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to_top(display_range),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to_bottom(viewport, display_range),
            _ => {}
        }
    }

    pub(super) fn render(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        display_range: Option<Range<usize>>,
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
        let range = self.visible_range(display_range.clone());
        let scroll = self.diff_scroll.clamp(range.start, range.end);
        let end = scroll.saturating_add(viewport.max(1)).min(range.end);
        let visible: Vec<Line<'static>> = self.diff_lines[scroll..end].to_vec();

        let footer = self.footer(display_range);
        let block = pane_block_with_footer("─[2]─Diff─", color, footer);
        frame.render_widget(Paragraph::new(visible).block(block), area);

        // Vertical scrollbar reflects the *visible range*, not the full
        // diff: when the sidebar is on a directory the scrollbar tracks
        // progress through that subtree's files.
        let span = range.end.saturating_sub(range.start);
        let position = scroll.saturating_sub(range.start);
        render_pane_scrollbar(frame, area, span, position, pane_inner_height(area), theme);
    }

    /// Build the diff pane's bottom-right footer: `" line X of Y "` for
    /// the current scroll position within the visible range, or `None`
    /// when the pane is empty.
    pub(super) fn footer(&self, display_range: Option<Range<usize>>) -> Option<String> {
        let range = self.visible_range(display_range);
        let span = range.end.saturating_sub(range.start);
        if span == 0 {
            return None;
        }
        let pos = self
            .diff_scroll
            .saturating_sub(range.start)
            .min(span.saturating_sub(1))
            + 1;
        Some(format!(" line {pos} of {span}  \u{00b7}  ? help "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::review::model::precompute_diffs;
    use crate::cli::review::test_support::*;
    use crate::cli::review::{Focus, handle_key};

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
    fn build_view_emits_file_header_and_hunk_for_one_file() {
        let f = file_diff("foo.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0], 80, &theme());
        let texts: Vec<String> = view.lines.iter().map(line_text).collect();

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
    fn build_view_records_one_offset_per_file() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0, 1], 80, &theme());
        assert_eq!(view.file_offsets.len(), 2);
        // First offset is 0 (no leading blank).
        assert_eq!(view.file_offsets[0], 0);
        // Second offset points at b's header line.
        let second = view.file_offsets[1];
        let header_text = line_text(&view.lines[second]);
        assert_eq!(header_text, "b.txt");
    }

    #[test]
    fn build_view_renders_in_display_order() {
        // Files supplied in input order [a, b], display order [b, a].
        // Output's first file header must be b's; offsets keyed by
        // input index.
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[1, 0], 80, &theme());
        assert_eq!(line_text(&view.lines[0]), "b.txt");
        assert_eq!(view.file_offsets[1], 0);
        assert!(view.file_offsets[0] > 0);
    }

    #[test]
    fn build_view_includes_rename_header_when_renamed() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(&resolved, &diffs, &[0], 80, &theme());
        let combined: String = view
            .lines
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
        // Stay in Sidebar focus; Shift+J should still scroll the diff.
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Char('J'), 4, 4);
        assert_eq!(state.diff.diff_scroll, SCROLL_STEP_LARGE);
    }

    #[test]
    fn dir_filter_excludes_files_outside_subtree() {
        // Three files under three different dirs. Each file's diff has
        // a unique marker line so we can assert exactly which files are
        // visible at any time.
        let a = file_diff("alpha/a.rs");
        let b = file_diff("beta/b.rs");
        let c = file_diff("gamma/c.rs");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "old_alpha\n".to_string(),
                after: "new_alpha\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "old_beta\n".to_string(),
                after: "new_beta\n".to_string(),
            },
            ResolvedFile {
                file: c,
                before: "old_gamma\n".to_string(),
                after: "new_gamma\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        // Walk to the `beta/` dir header. Tree order: alpha/ (dir 0),
        // alpha/a.rs (file 0), beta/ (dir 1), beta/b.rs (file 1),
        // gamma/ (dir 2), gamma/c.rs (file 2).
        // Initial selection is on file 0 (alpha/a.rs at row 1).
        // Step down to row 2 = beta/.
        state.sidebar.move_down(20);
        assert!(state.sidebar.selected_is_dir());

        let range = state.visible_diff_range();
        let visible_text: String = state.diff.diff_lines[range.clone()]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        // Only beta/b.rs's content must be inside the range.
        assert!(
            visible_text.contains("beta/b.rs")
                && visible_text.contains("old_beta")
                && visible_text.contains("new_beta"),
            "beta content missing from filtered range: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("alpha/a.rs") && !visible_text.contains("old_alpha"),
            "alpha leaked into beta filter: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("gamma/c.rs") && !visible_text.contains("old_gamma"),
            "gamma leaked into beta filter: {visible_text:?}"
        );

        // Move to a file row — visible range narrows to that single file.
        state.sidebar.move_down(20); // file row inside beta/
        assert!(!state.sidebar.selected_is_dir());
        let file_range = state.visible_diff_range();
        let file_text: String = state.diff.diff_lines[file_range]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            file_text.contains("beta/b.rs") && file_text.contains("new_beta"),
            "beta file content missing from filtered range: {file_text:?}"
        );
        assert!(
            !file_text.contains("alpha/a.rs") && !file_text.contains("gamma/c.rs"),
            "siblings leaked into single-file filter: {file_text:?}"
        );
    }

    #[test]
    fn visible_diff_range_narrows_to_subtree_on_dir_selection() {
        // Two files in different dirs: src/a.rs and other/b.rs. The
        // sidebar tree puts each under its own dir header. Selecting
        // src/ should restrict visible_diff_range to just src/a.rs's
        // lines; selecting other/ should restrict to other/b.rs.
        let a = file_diff("src/a.rs");
        let b = file_diff("other/b.rs");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);

        // Initial selection is on a file: visible range is exactly
        // that file (single-element subset of the full diff).
        let file_range = state.visible_diff_range();
        assert!(
            file_range.end - file_range.start < state.diff.diff_lines.len(),
            "file selection should narrow to a single file's slice"
        );
        let file_first_line = line_text(&state.diff.diff_lines[file_range.start]);
        assert!(
            file_first_line == "src/a.rs" || file_first_line == "other/b.rs",
            "expected a file header at start, got {file_first_line:?}"
        );

        // Move up onto the dir header above the first file (other/ is
        // first alphabetically among the directory rows).
        state.sidebar.top(20);
        assert!(state.sidebar.selected_is_dir());
        let narrowed = state.visible_diff_range();
        // The range must be strictly smaller than the full diff.
        assert!(
            narrowed.end - narrowed.start < state.diff.diff_lines.len(),
            "expected subtree range to be narrower than full diff"
        );
        // The very first visible line should be the file header for
        // whichever file the dir contains.
        let first_line = line_text(&state.diff.diff_lines[narrowed.start]);
        assert!(
            first_line == "src/a.rs" || first_line == "other/b.rs",
            "expected dir's file header at start, got {first_line:?}"
        );
    }

    #[test]
    fn diff_footer_includes_help_hint() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let state = make_state(&resolved);
        let footer = state
            .diff
            .footer(state.sidebar.selection_display_range())
            .expect("footer present");
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
        state.focus = Focus::Diff;
        let before = state.diff.diff_scroll;

        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollDown, 50, 5);
        crate::cli::review::handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff.diff_scroll > before);

        let after_down = state.diff.diff_scroll;
        let mouse = make_mouse(crossterm::event::MouseEventKind::ScrollUp, 50, 5);
        crate::cli::review::handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.diff.diff_scroll < after_down);
    }
}
