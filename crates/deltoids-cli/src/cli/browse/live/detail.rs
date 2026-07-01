//! Detail/diff pane slice: the cached rendered diff for the selected
//! feed entry, plus the pane render.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use deltoids::Theme;
use deltoids::render_tui::{
    self, pane_block, pane_border_color, render_pane_scrollbar, rgb_to_color,
};

use crate::cli::browse::files::model::count_deltas;

use super::Focus;
use super::model::FeedEntry;

/// Rendered diff lines for one feed entry, valid for a given width.
#[derive(Debug, Clone)]
pub(super) struct DiffCache {
    pub(super) entry_index: usize,
    pub(super) width: usize,
    pub(super) lines: Vec<Line<'static>>,
}

/// Maximum scroll offset that still keeps content in view.
pub(super) fn max_detail_scroll(row_count: usize, height: usize) -> usize {
    row_count.saturating_sub(height.max(1))
}

/// Render one feed entry as a flat line stream: a file header, a metadata
/// line (time and +added/-removed counts), then each hunk.
pub(super) fn render_detail_for(
    entry: &FeedEntry,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = render_tui::render_file_header(&entry.path, width, theme);

    let (added, removed) = count_deltas(&entry.diff);
    lines.push(Line::from(vec![
        Span::styled(
            entry.timestamp.clone(),
            Style::default().fg(rgb_to_color(theme.muted)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("+{added}"),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{removed}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ]));

    for hunk in entry.diff.hunks() {
        lines.push(Line::from(""));
        lines.extend(render_tui::render_hunk(
            hunk,
            entry.diff.highlight(),
            width,
            theme,
        ));
    }
    lines
}

/// Render the diff pane for `entry`, using and refreshing `cache`.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_diff_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    entry: Option<&FeedEntry>,
    entry_index: usize,
    cache: &mut Option<DiffCache>,
    diff_scroll: usize,
    focus: Focus,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(2) as usize;
    let color = pane_border_color(focus == Focus::Diff, theme);

    let Some(entry) = entry else {
        frame.render_widget(pane_block("─[2]─Diff─", color), area);
        return;
    };

    let valid = cache
        .as_ref()
        .is_some_and(|c| c.entry_index == entry_index && c.width == width);
    if !valid {
        *cache = Some(DiffCache {
            entry_index,
            width,
            lines: render_detail_for(entry, width, theme),
        });
    }

    let row_count = cache.as_ref().map(|c| c.lines.len()).unwrap_or(0);
    let viewport = area.height.saturating_sub(2) as usize;
    let start = diff_scroll.min(row_count);
    let end = start.saturating_add(viewport.max(1)).min(row_count);
    let visible: Vec<Line<'static>> = cache
        .as_ref()
        .map(|c| c.lines[start..end].to_vec())
        .unwrap_or_default();

    frame.render_widget(
        Paragraph::new(visible).block(pane_block("─[2]─Diff─", color)),
        area,
    );
    render_pane_scrollbar(frame, area, row_count, diff_scroll, viewport, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::Diff;

    fn entry(path: &str, before: &str, after: &str) -> FeedEntry {
        FeedEntry {
            path: path.to_string(),
            timestamp: "12:00:00".to_string(),
            diff: Diff::compute(before, after, path),
        }
    }

    #[test]
    fn render_detail_includes_header_metadata_and_body() {
        let theme = Theme::default();
        let e = entry("a.txt", "one\n", "one\ntwo\n");
        let lines = render_detail_for(&e, 80, &theme);
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("a.txt"), "missing header: {text}");
        assert!(text.contains("12:00:00"), "missing timestamp: {text}");
        assert!(text.contains("+1"), "missing added count: {text}");
        assert!(text.contains("two"), "missing added line: {text}");
    }

    #[test]
    fn max_detail_scroll_clamps_to_zero_when_content_fits() {
        assert_eq!(max_detail_scroll(3, 10), 0);
        assert_eq!(max_detail_scroll(20, 10), 10);
    }
}
