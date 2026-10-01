//! Change overview beside the diff pane's right-border scrollbar.

use deltoids::render_tui::{render_pane_scrollbar, rgb_to_color};
use deltoids::{LineKind, Theme};
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Style};

use super::diff_cursor::DiffRow;

/// Reserve one inner column when at least one column remains for text.
/// Reservation is independent of overflow, so wrapping cannot change it.
pub(super) fn body_width(inner_width: usize) -> usize {
    inner_width.saturating_sub(usize::from(inner_width >= 2))
}

pub(super) fn body_area(area: Rect) -> Rect {
    let mut inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    inner.width = body_width(inner.width as usize) as u16;
    inner
}

/// Paint all assembled rows, including comments and wrapped continuations,
/// into a height-sized overview. Collisions retain both kinds of change.
/// The scrollbar thumb uses scroll offsets; marks use full-content positions.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[DiffRow],
    scroll: usize,
    focused: bool,
    theme: &Theme,
) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let height = inner.height as usize;
    if height == 0 || inner.width == 0 {
        return;
    }
    render_pane_scrollbar(frame, area, rows.len(), scroll, height, focused, theme);
    if inner.width < 2 {
        return;
    }

    let mut cells = vec![(false, false); height];
    for (index, row) in rows.iter().enumerate() {
        let cell = if rows.len() <= height {
            index
        } else {
            // Use the endpoints of both ranges so the last row reaches
            // the bottom. u128 keeps multiplication safe on large inputs.
            ((index as u128 * (height - 1) as u128) / (rows.len() - 1) as u128) as usize
        };
        match row.kind {
            Some(LineKind::Added) => cells[cell].0 = true,
            Some(LineKind::Removed) => cells[cell].1 = true,
            _ => {}
        }
    }
    let added = rgb_to_color(theme.status_added);
    let removed = rgb_to_color(theme.status_deleted);
    let x = inner.right() - 1;
    for (offset, (has_added, has_removed)) in cells.into_iter().enumerate() {
        let (symbol, style) = match (has_added, has_removed) {
            (true, true) => ("▀", Style::default().fg(added).bg(removed)),
            (true, false) => ("█", Style::default().fg(added).bg(Color::Reset)),
            (false, true) => ("█", Style::default().fg(removed).bg(Color::Reset)),
            (false, false) => (" ", Style::default().fg(Color::Reset).bg(Color::Reset)),
        };
        frame.buffer_mut()[(x, inner.y + offset as u16)]
            .set_symbol(symbol)
            .set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::text::Line;

    fn render_rows(kinds: &[Option<LineKind>], scroll: usize, width: u16, height: u16) -> Buffer {
        let rows: Vec<_> = kinds
            .iter()
            .map(|kind| {
                let mut row = DiffRow::plain(Line::from(""));
                row.kind = kind.clone();
                row
            })
            .collect();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &rows, scroll, true, &Theme::default()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn offscreen_changes_remain_visible_at_both_scroll_extremes() {
        let mut kinds = vec![None; 101];
        kinds[0] = Some(LineKind::Removed);
        kinds[50] = Some(LineKind::Added);
        kinds[100] = Some(LineKind::Removed);
        let theme = Theme::default();
        let top = render_rows(&kinds, 0, 10, 7);
        let bottom = render_rows(&kinds, 96, 10, 7);
        for buffer in [&top, &bottom] {
            assert_eq!(buffer[(8, 1)].fg, rgb_to_color(theme.status_deleted));
            assert_eq!(buffer[(8, 3)].fg, rgb_to_color(theme.status_added));
            assert_eq!(buffer[(8, 5)].fg, rgb_to_color(theme.status_deleted));
        }
        assert_eq!(top[(9, 1)].symbol(), "▐");
        assert_eq!(bottom[(9, 5)].symbol(), "▐");
        assert_ne!(top[(9, 5)].symbol(), "▐");
        assert_ne!(bottom[(9, 1)].symbol(), "▐");
    }

    #[test]
    fn compressed_changes_show_both_colors_instead_of_losing_one() {
        let buffer = render_rows(
            &[Some(LineKind::Added), Some(LineKind::Removed), None, None],
            0,
            10,
            4,
        );
        let theme = Theme::default();
        assert_eq!(buffer[(8, 1)].symbol(), "▀");
        assert_eq!(buffer[(8, 1)].fg, rgb_to_color(theme.status_added));
        assert_eq!(buffer[(8, 1)].bg, rgb_to_color(theme.status_deleted));
    }

    #[test]
    fn fitting_content_aligns_marks_with_rows_without_a_thumb() {
        let buffer = render_rows(
            &[
                Some(LineKind::Added),
                Some(LineKind::Context),
                None,
                Some(LineKind::Removed),
            ],
            0,
            10,
            8,
        );
        assert_eq!(buffer[(8, 1)].symbol(), "█");
        assert_eq!(buffer[(8, 2)].symbol(), " ");
        assert_eq!(buffer[(8, 3)].symbol(), " ");
        assert_eq!(buffer[(8, 4)].symbol(), "█");
        assert_eq!(buffer[(8, 5)].symbol(), " ");
        assert_eq!(buffer[(9, 1)].symbol(), " ");
    }

    #[test]
    fn tiny_panes_leave_body_space_and_render_without_panicking() {
        for width in 0..5 {
            for height in 0..5 {
                let area = Rect::new(0, 0, width, height);
                let body = body_area(area);
                let inner = area.inner(Margin {
                    vertical: 1,
                    horizontal: 1,
                });
                assert_eq!(body.width as usize, body_width(inner.width as usize));
                let _ = render_rows(&[Some(LineKind::Added)], 0, width, height);
            }
        }
    }
}
