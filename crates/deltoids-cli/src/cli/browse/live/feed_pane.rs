//! Feed pane slice: the scrolling list of feed entries (newest last),
//! their row labels, and the pane render.

use std::path::Path;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem},
};

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_tabs, pane_border_color, pane_inner_height, position_footer,
    render_pane_scrollbar, rgb_to_color,
};

use crate::cli::browse::files::model::count_deltas;

use super::model::FeedEntry;

fn file_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// One feed row: `HH:MM:SS  basename  +A -D`.
fn entry_label_line(entry: &FeedEntry) -> Line<'static> {
    let (added, removed) = count_deltas(&entry.diff);
    Line::from(vec![
        Span::styled(
            format!("{} ", entry.timestamp),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(format!("{} ", file_basename(&entry.path))),
        Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(Color::Red)),
    ])
}

/// Plain-text feed row label (for tests / non-styled contexts).
#[cfg(test)]
pub(super) fn entry_label_plain(entry: &FeedEntry) -> String {
    let (added, removed) = count_deltas(&entry.diff);
    format!(
        "{} {} +{added} -{removed}",
        entry.timestamp,
        file_basename(&entry.path)
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_feed_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    entries: &[FeedEntry],
    selected: usize,
    list_state: &mut ratatui::widgets::ListState,
    focused: bool,
    title: Line<'static>,
    theme: &Theme,
) {
    let items = entries
        .iter()
        .map(|entry| ListItem::new(entry_label_line(entry)))
        .collect::<Vec<_>>();
    let count = entries.len();
    let position = if count == 0 { 0 } else { selected + 1 };
    let list = List::new(items)
        .block(pane_block_with_tabs(
            title,
            pane_border_color(focused, theme),
            Some(position_footer(position, count)),
        ))
        .highlight_style(
            Style::default()
                .bg(rgb_to_color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .scroll_padding(2);
    frame.render_stateful_widget(list, area, list_state);
    render_pane_scrollbar(frame, area, count, selected, pane_inner_height(area), theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::Diff;

    fn entry(path: &str, before: &str, after: &str) -> FeedEntry {
        FeedEntry {
            path: path.to_string(),
            timestamp: "09:41:00".to_string(),
            diff: Diff::compute(before, after, path),
        }
    }

    #[test]
    fn label_shows_time_basename_and_counts() {
        let e = entry("src/main.rs", "a\n", "a\nb\n");
        let plain = entry_label_plain(&e);
        assert!(plain.contains("09:41:00"), "missing time: {plain}");
        assert!(plain.contains("main.rs"), "missing basename: {plain}");
        assert!(plain.contains("+1"), "missing added count: {plain}");
    }
}
