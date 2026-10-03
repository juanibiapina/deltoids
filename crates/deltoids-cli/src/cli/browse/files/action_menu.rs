use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

use super::actions::{DiscardKind, PendingDiscard};

#[derive(Debug)]
pub(super) enum DiscardMenu {
    Checking,
    Ready(PendingDiscard),
    Unavailable(String),
}

impl DiscardMenu {
    pub(super) fn is_available(&self, kind: DiscardKind) -> bool {
        matches!(self, Self::Ready(pending) if pending.is_available(kind))
    }
}

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    menu: &DiscardMenu,
    selected: usize,
    theme: &Theme,
) {
    let area = frame.area();
    let width = area.width.min(80);
    let height = area.height.min(10);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let label = match menu {
        DiscardMenu::Ready(pending) => pending.label(),
        _ => String::new(),
    };
    let mut lines = vec![Line::from(label), Line::from("")];
    let labels = DiscardKind::ALL
        .iter()
        .map(|kind| kind.label())
        .chain(["Cancel"]);
    for (index, label) in labels.enumerate() {
        let mut style = Style::default();
        if DiscardKind::ALL
            .get(index)
            .is_some_and(|kind| !menu.is_available(*kind))
        {
            style = style
                .fg(rgb_to_color(theme.muted))
                .add_modifier(Modifier::CROSSED_OUT);
        }
        if index == selected {
            style = style.bg(rgb_to_color(theme.selection_bg));
        }
        lines.push(Line::from(Span::styled(
            format!("{} {label}", if index == selected { "›" } else { " " }),
            style,
        )));
    }
    lines.push(Line::from(""));
    let description = match menu {
        DiscardMenu::Checking => "Checking discard…",
        DiscardMenu::Unavailable(reason) => reason,
        DiscardMenu::Ready(pending) if !pending.is_available(DiscardKind::All) => {
            "No changes to discard"
        }
        DiscardMenu::Ready(_) => DiscardKind::ALL
            .get(selected)
            .map_or("", |kind| kind.description()),
    };
    lines.push(Line::from(description));
    frame.render_widget(Clear, popup);
    let block = pane_block("─Discard changes─", pane_border_color(true, theme));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}
