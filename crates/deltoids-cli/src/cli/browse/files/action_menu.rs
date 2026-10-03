use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

use super::actions::PendingDiscard;

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    pending: &PendingDiscard,
    selected: usize,
    theme: &Theme,
) {
    let area = frame.area();
    let width = area.width.min(80);
    let height = area.height.min(11);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let mut lines = vec![Line::from(pending.label()), Line::from("")];
    let labels = pending
        .choices()
        .iter()
        .map(|kind| kind.label())
        .chain(["Cancel"]);
    for (index, label) in labels.enumerate() {
        let style = if index == selected {
            Style::default().bg(rgb_to_color(theme.selection_bg))
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{} {label}", if index == selected { "›" } else { " " }),
            style,
        )));
    }
    lines.push(Line::from(""));
    if let Some(kind) = pending.choices().get(selected) {
        lines.push(Line::from(kind.description()));
    }
    lines.push(Line::from(Span::styled(
        "Enter: select · Esc: cancel",
        Style::default().fg(rgb_to_color(theme.muted)),
    )));
    frame.render_widget(Clear, popup);
    let block = pane_block("─Discard changes─", pane_border_color(true, theme));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}
