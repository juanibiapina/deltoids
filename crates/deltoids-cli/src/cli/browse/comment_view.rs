//! Presentation of review comments: the inline rows drawn under a
//! commented diff line, the cursor highlight, and the single-line editor
//! popup. Shared by both modes, and kept apart from [`super::comments`]
//! (the pure store/prompt core) and from the diff panes themselves.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use deltoids::Theme;
use deltoids::render_tui::{pane_block, rgb_to_color};

use super::comments::{Comment, CommentAnchor, CommentStore};
use super::diff_cursor::DiffRow;
use super::text::{display_width, wrap_text};

/// Marker drawn at the start of a comment's first row.
const COMMENT_MARKER: &str = "\u{258c} ";

/// Draw the saved comments into `rows`, each under the line it annotates.
///
/// Comments are an overlay, not part of the diff: panes cache the
/// expensive syntax-highlighted rows and splice comments in on the way to
/// the screen. That is what keeps writing or deleting a note from
/// re-rendering (and briefly blanking) the file it belongs to.
///
/// `is_outdated` decides whether a note still describes the line it points
/// at. A pane whose diff cannot change under the reviewer (a recorded
/// trace entry) passes a closure that always answers `false`.
pub(in crate::cli::browse) fn with_comments(
    rows: &[DiffRow],
    comments: &CommentStore,
    width: usize,
    theme: &Theme,
    is_outdated: impl Fn(&CommentAnchor, &Comment) -> bool,
) -> Vec<DiffRow> {
    if comments.is_empty() {
        return rows.to_vec();
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let comment = row
            .ends_line
            .then_some(row.anchor.as_ref())
            .flatten()
            .and_then(|anchor| comments.get(anchor).map(|comment| (anchor, comment)));
        out.push(row.clone());
        if let Some((anchor, comment)) = comment {
            let outdated = is_outdated(anchor, comment);
            out.extend(
                render_comment_rows(&comment.note, width, theme, outdated)
                    .into_iter()
                    .map(DiffRow::plain),
            );
        }
    }
    out
}

/// Render a saved comment as accent-coloured, marker-prefixed rows
/// wrapped to `width`. Drawn directly under the line it annotates.
///
/// An `outdated` comment is one whose line no longer reads the way it did
/// when the note was written (the working tree moved on in a way the
/// re-anchor pass could not follow). It is dimmed and labelled, so a note
/// about old code never passes for a note about the code on screen.
pub(in crate::cli::browse) fn render_comment_rows(
    text: &str,
    width: usize,
    theme: &Theme,
    outdated: bool,
) -> Vec<Line<'static>> {
    let text = if outdated {
        format!("{text}  \u{b7} outdated")
    } else {
        text.to_string()
    };
    let accent = Style::default().fg(rgb_to_color(if outdated {
        theme.muted
    } else {
        theme.border_active
    }));
    let body_width = width.saturating_sub(COMMENT_MARKER.chars().count()).max(1);
    wrap_text(&text, body_width)
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let prefix = if index == 0 { COMMENT_MARKER } else { "  " };
            Line::from(vec![
                Span::styled(prefix.to_string(), accent),
                Span::styled(segment, accent),
            ])
        })
        .collect()
}

/// Repaint `line` with the cursor background, padded to the pane width so
/// the highlight spans the whole row.
pub(in crate::cli::browse) fn highlight_row(
    line: Line<'static>,
    width: usize,
    bg: Color,
) -> Line<'static> {
    let used: usize = line
        .spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum();
    let mut spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let style = span.style.bg(bg);
            Span::styled(span.content, style)
        })
        .collect();
    if width > used {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }
    Line::from(spans)
}

/// Draw the comment editor as a centered popup over the diff pane:
/// the target `label` (`path:line`) above the text being typed.
pub(in crate::cli::browse) fn render_comment_editor(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    label: &str,
    buffer: &str,
    theme: &Theme,
) {
    let popup = centered_rect(area, 70, 7);
    let lines = vec![
        Line::from(Span::styled(
            label.to_string(),
            Style::default()
                .fg(rgb_to_color(theme.muted))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("{buffer}\u{2588}")),
    ];
    let paragraph = Paragraph::new(lines)
        .block(pane_block(
            "─Comment (Enter save · Esc cancel)─",
            rgb_to_color(theme.border_active),
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

/// Center a `width_pct`-wide, `height`-tall rect inside `area`.
fn centered_rect(area: Rect, width_pct: u16, height: u16) -> Rect {
    let popup_width = (area.width * width_pct / 100).max(20).min(area.width);
    let popup_height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(x, y, popup_width, popup_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::Theme;

    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn comment_rows_mark_the_first_row_and_indent_the_rest() {
        let theme = Theme::default();
        let rows = render_comment_rows("alpha beta gamma delta", 12, &theme, false);
        assert!(rows.len() > 1, "long comments wrap");
        assert!(text_of(&rows[0]).starts_with(COMMENT_MARKER));
        assert!(text_of(&rows[1]).starts_with("  "));
        // Every row is accent-coloured.
        let accent = rgb_to_color(theme.border_active);
        assert!(
            rows.iter()
                .all(|row| row.spans.iter().all(|s| s.style.fg == Some(accent)))
        );
    }

    #[test]
    fn an_outdated_comment_is_labelled_and_dimmed() {
        let theme = Theme::default();
        let rows = render_comment_rows("fix this", 40, &theme, true);
        let text: String = rows.iter().map(text_of).collect();
        assert!(text.contains("fix this"));
        assert!(
            text.contains("outdated"),
            "outdated comments say so: {text:?}"
        );
        assert!(
            rows.iter().all(|row| row
                .spans
                .iter()
                .all(|s| s.style.fg == Some(rgb_to_color(theme.muted)))),
            "outdated comments are dimmed"
        );
    }

    #[test]
    fn highlight_row_pads_to_the_pane_width() {
        let highlighted = highlight_row(Line::from("abc"), 10, Color::Blue);
        assert_eq!(text_of(&highlighted).chars().count(), 10);
        assert!(
            highlighted
                .spans
                .iter()
                .all(|span| span.style.bg == Some(Color::Blue))
        );
    }

    #[test]
    fn centered_rect_fits_inside_small_areas() {
        let area = Rect::new(0, 0, 10, 3);
        let popup = centered_rect(area, 70, 7);
        assert!(popup.width <= area.width);
        assert!(popup.height <= area.height);
    }
}
