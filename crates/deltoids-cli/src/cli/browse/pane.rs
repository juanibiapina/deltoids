//! A pane of the two columns: a title row over a body, with no frame.
//!
//! The title row names the pane on the left and carries badges and a
//! footer at its right end. A rule under it sets it off from the body. The pane's last column, on every row, is
//! its scrollbar's, so a pane whose last column is the divider scrolls
//! along it. A message, while there is one, covers the right end of
//! the last row. Callers draw into [`body`] and never offset by hand.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::scrollbar as scrollbar_symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use deltoids::Theme;
use deltoids::render_tui::{pane_border_color, rgb_to_color};

/// What a pane shows around its body.
pub(crate) struct Chrome {
    /// The pane's name, or its tabs, in the focus colour unless a span
    /// says otherwise.
    pub title: Line<'static>,
    /// Marks at the right end of the title row, before the footer.
    pub badges: Vec<Span<'static>>,
    /// The rightmost text of the title row. Dropped when it does not
    /// fit beside the whole title.
    pub footer: Option<Line<'static>>,
    pub focused: bool,
}

impl Chrome {
    pub(crate) fn new(title: Line<'static>, focused: bool) -> Self {
        Self {
            title,
            badges: Vec::new(),
            footer: None,
            focused,
        }
    }

    pub(crate) fn footer(mut self, footer: Option<Line<'static>>) -> Self {
        self.footer = footer;
        self
    }

    pub(crate) fn badges(mut self, badges: Vec<Span<'static>>) -> Self {
        self.badges = badges;
        self
    }
}

/// Columns of the title kept when the badges need the room.
const TITLE_FLOOR: usize = 12;

/// Rows above a pane's body: the title row and the rule under it.
const HEAD_ROWS: u16 = 2;

/// Where a pane's content goes: below the title row and its rule, one
/// column in from the pane's left edge, left of the scrollbar column.
pub(crate) fn body(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1).min(area.right()),
        y: area.y.saturating_add(HEAD_ROWS).min(area.bottom()),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(HEAD_ROWS),
    }
}

/// Rows of content a pane of `area` shows.
pub(crate) fn body_height(area: Rect) -> usize {
    body(area).height as usize
}

/// Draw the title row, over the body's columns, and return the body.
pub(crate) fn draw(frame: &mut Frame<'_>, area: Rect, chrome: Chrome, theme: &Theme) -> Rect {
    let row = Rect {
        height: area.height.min(1),
        ..body(area)
    };
    let row = Rect { y: area.y, ..row };
    if row.width == 0 || row.height == 0 {
        return body(area);
    }
    let width = usize::from(row.width);
    let title_width = chrome.title.width();
    let badges = right_end(chrome.badges.clone(), None);
    let with_footer = right_end(chrome.badges, chrome.footer);
    let right = if title_width + 1 + with_footer.width() <= width {
        with_footer
    } else {
        badges
    };
    let room = width.saturating_sub(title_width.min(TITLE_FLOOR) + 1);
    let right_width = if right.width() <= room {
        right.width() as u16
    } else {
        0
    };
    let gap = u16::from(right_width > 0);
    let title_area = Rect {
        width: row.width.saturating_sub(right_width + gap),
        ..row
    };
    let color = pane_border_color(chrome.focused, theme);
    frame.render_widget(
        Paragraph::new(chrome.title).style(Style::default().fg(color)),
        title_area,
    );
    if right_width > 0 {
        let right_area = Rect {
            x: row.right() - right_width,
            width: right_width,
            ..row
        };
        let muted = Style::default().fg(rgb_to_color(theme.muted));
        frame.render_widget(Paragraph::new(right).style(muted), right_area);
    }
    if area.height > 1 {
        let style = Style::default().fg(rgb_to_color(theme.border));
        let rule = "\u{2500}".repeat(usize::from(area.width));
        frame
            .buffer_mut()
            .set_string(area.x, area.y + 1, rule, style);
    }
    body(area)
}

/// Feedback for the last action at the right end of the pane's last
/// row. Draw it after the body, which it covers.
pub(crate) fn draw_message(
    frame: &mut Frame<'_>,
    area: Rect,
    message: Line<'static>,
    theme: &Theme,
) {
    let body = body(area);
    if body.height == 0 {
        return;
    }
    let width = (message.width() as u16).min(body.width);
    let row = Rect {
        x: body.right() - width,
        y: body.bottom() - 1,
        width,
        height: 1,
    };
    let style = Style::default().bg(rgb_to_color(theme.selection_bg));
    frame.render_widget(Clear, row);
    frame.render_widget(Paragraph::new(message).style(style), row);
}

/// The badges, then the footer, a space apart.
fn right_end(mut spans: Vec<Span<'static>>, footer: Option<Line<'static>>) -> Line<'static> {
    if let Some(footer) = footer {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.extend(footer.spans);
    }
    Line::from(spans)
}

/// Draw a vertical scrollbar in the pane's last column, beside the
/// body: the track in the divider's colour, the thumb in the focus
/// colour. No-op when `content_length` rows fit.
/// `position` is the top row's index, or the selected row's in a list.
pub(crate) fn draw_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_length: usize,
    position: usize,
    focused: bool,
    theme: &Theme,
) {
    if area.width == 0 {
        return;
    }
    let x = area.right() - 1;
    draw_scrollbar_beside(
        frame,
        body(area),
        x,
        content_length,
        position,
        focused,
        theme,
    );
}

/// [`draw_scrollbar`] for a list drawn into `body`, a part of a pane,
/// with the scrollbar in column `x` beside it.
pub(crate) fn draw_scrollbar_beside(
    frame: &mut Frame<'_>,
    body: Rect,
    x: u16,
    content_length: usize,
    position: usize,
    focused: bool,
    theme: &Theme,
) {
    let viewport = body.height as usize;
    if content_length <= viewport.max(1) {
        return;
    }
    let color = pane_border_color(focused, theme);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(scrollbar_symbols::VERTICAL)
        .thumb_symbol("\u{2590}")
        .track_style(Style::default().fg(rgb_to_color(theme.border)))
        .thumb_style(Style::default().fg(color))
        .begin_symbol(None)
        .end_symbol(None);
    // Ratatui puts the thumb at the track bottom only when position ==
    // content_length - 1. Scroll offsets max out at content_length -
    // viewport, so pass max_scroll + 1 as the length and clamp the
    // position: the thumb reaches the bottom for offsets and selections.
    let max_scroll = content_length.saturating_sub(viewport);
    let mut state = ScrollbarState::new(max_scroll.saturating_add(1))
        .position(position.min(max_scroll))
        .viewport_content_length(viewport);
    let column = Rect {
        x,
        width: 1,
        ..body
    };
    frame.render_stateful_widget(scrollbar, column, &mut state);
}

/// Join the panes' rules across the divider at column `x` and the gap
/// after it. Draw it after the panes.
pub(crate) fn draw_junction(frame: &mut Frame<'_>, x: u16, area: Rect, theme: &Theme) {
    if area.height < 2 || x >= area.right() {
        return;
    }
    let style = Style::default().fg(rgb_to_color(theme.border));
    let y = area.y + 1;
    let buffer = frame.buffer_mut();
    buffer[(x, y)].set_symbol("\u{253c}").set_style(style);
    if x + 1 < area.right() {
        buffer[(x + 1, y)].set_symbol("\u{2500}").set_style(style);
    }
}

/// Draw the divider between the two columns at column `x`, before
/// the panes: the side panel's scrollbar draws over it.
pub(crate) fn draw_divider(frame: &mut Frame<'_>, x: u16, area: Rect, theme: &Theme) {
    if x >= area.right() {
        return;
    }
    let style = Style::default().fg(rgb_to_color(theme.border));
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        buffer[(x, y)].set_symbol("\u{2502}").set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The title row of a pane `width` wide, between its left inset and
    /// its scrollbar column.
    fn row(chrome: Chrome, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, frame.area(), chrome, &Theme::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), " ");
        assert_eq!(buffer[(width - 1, 0)].symbol(), " ");
        (1..width - 1).map(|x| buffer[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_title_row_names_the_pane_and_ends_with_badges_and_footer() {
        let chrome = Chrome::new(Line::from("Diff"), true)
            .badges(vec![Span::raw("Rust")])
            .footer(Some(Line::from("? help")));
        assert_eq!(row(chrome, 26), "Diff         Rust ? help");
    }

    #[test]
    fn a_narrow_row_drops_the_footer_and_shortens_the_title_for_badges() {
        let chrome = || {
            Chrome::new(Line::from("src/cli/browse/files/mod.rs"), true)
                .badges(vec![Span::raw("Rust")])
                .footer(Some(Line::from("? help")))
        };
        assert_eq!(row(chrome(), 34), "src/cli/browse/files/mod.rs Rust");
        assert_eq!(row(chrome(), 22), "src/cli/browse/ Rust");
        assert_eq!(row(chrome(), 17), "src/cli/browse/");
    }

    #[test]
    fn the_thumb_takes_the_focus_colour_and_the_track_the_dividers() {
        let theme = Theme::default();
        let thumb_and_track = |focused: bool| {
            let mut terminal = Terminal::new(TestBackend::new(6, 8)).unwrap();
            terminal
                .draw(|frame| {
                    draw_scrollbar(frame, frame.area(), 100, 0, focused, &Theme::default());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let cells: Vec<_> = (1..8).map(|y| buffer[(5, y)].clone()).collect();
            let thumb = cells.iter().find(|c| c.symbol() == "\u{2590}").unwrap().fg;
            let track = cells.iter().find(|c| c.symbol() == "\u{2502}").unwrap().fg;
            (thumb, track)
        };
        let divider = rgb_to_color(theme.border);
        assert_eq!(
            thumb_and_track(true),
            (pane_border_color(true, &theme), divider)
        );
        assert_eq!(
            thumb_and_track(false),
            (pane_border_color(false, &theme), divider)
        );
    }

    #[test]
    fn a_message_covers_the_end_of_the_last_row() {
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                let body = draw(
                    frame,
                    area,
                    Chrome::new(Line::from("Diff"), true),
                    &Theme::default(),
                );
                frame.render_widget(
                    Paragraph::new("x".repeat(30)).wrap(ratatui::widgets::Wrap { trim: false }),
                    body,
                );
                draw_message(frame, area, Line::from("Copied"), &Theme::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let last: String = (0..10).map(|x| buffer[(x, 2)].symbol()).collect();
        assert_eq!(last, " xxCopied ");
    }

    #[test]
    fn the_body_sits_below_the_title_between_the_inset_and_the_scrollbar() {
        let area = Rect::new(3, 2, 10, 6);
        assert_eq!(body(area), Rect::new(4, 4, 8, 4));
        assert_eq!(body_height(Rect::new(0, 0, 0, 0)), 0);
    }
}
