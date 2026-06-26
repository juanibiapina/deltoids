//! Shared help popup: the key table that is the source of truth for
//! bindings, the popup's own key handling, and its centered overlay
//! render. Mode-agnostic: the unified shell owns help visibility and
//! drives this slice for whichever mode is active.

use crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

use super::mode::AppCommand;

/// Single source of truth for the help popup. Each entry is
/// `(keys, description)` and renders as one row of a two-column
/// table inside the popup.
pub(super) const HELP_KEYS: &[(&str, &str)] = &[
    ("?", "toggle this help"),
    ("[ / ]", "switch Files / Traces mode"),
    ("Tab / 1 / 2 / 3", "focus panes in the current mode"),
    ("j / k", "move (list) or scroll one line (diff)"),
    ("Shift+J / K", "scroll diff three lines (any focus)"),
    ("PgDn / PgUp", "page in current pane"),
    ("g / G", "top / bottom of current pane"),
    ("Home / End", "top / bottom of current pane"),
    ("< / >", "narrow / widen sidebar (shared by modes)"),
    ("q / Esc", "quit (or close this popup)"),
];

/// Key dispatch while the help popup is shown. `?`, `Esc`, and `q`
/// all close the popup; everything else is swallowed. `q`/`Esc` do
/// **not** quit the app while the popup is open; closing the modal
/// first matches lazygit/k9s/vim convention.
pub(super) fn handle_key_help(help_visible: &mut bool, key: KeyCode) -> AppCommand {
    match key {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
            *help_visible = false;
        }
        _ => {}
    }
    AppCommand::Continue
}

/// Render the help popup as a centered, bordered overlay. Sized to
/// content (capped at 80% of the terminal in each axis), cleared
/// underneath so the panes don't bleed through. Pane chrome reuses
/// [`pane_block`] for visual consistency with the rest of the UI.
pub(super) fn draw_help_popup(frame: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    let rows = build_help_lines(theme);
    let content_width = HELP_KEYS
        .iter()
        .map(|(_k, d)| help_key_column_width().saturating_add(d.width()) + 2)
        .max()
        .unwrap_or(40);
    let want_w = (content_width as u16).saturating_add(4); // 2 borders + 2 padding
    let want_h = (rows.len() as u16).saturating_add(2); // 2 borders
    let max_w = (area.width * 8 / 10).max(20);
    let max_h = (area.height * 8 / 10).max(5);
    let w = want_w.min(max_w).min(area.width);
    let h = want_h.min(max_h).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, popup);
    let block = pane_block("─Help─", pane_border_color(true, theme));
    let inner = block.inner(popup).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(rows), inner);
}

/// Width reserved for the key column in the help popup so the
/// description column lines up.
fn help_key_column_width() -> usize {
    HELP_KEYS.iter().map(|(k, _)| k.width()).max().unwrap_or(0)
}

/// Build the help popup's body as ratatui [`Line`]s. Two columns:
/// keys (theme-accent) and description (default).
fn build_help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let key_w = help_key_column_width();
    let key_style = Style::default().fg(rgb_to_color(theme.border_active));
    let desc_style = Style::default().fg(rgb_to_color(theme.muted));
    HELP_KEYS
        .iter()
        .map(|(k, d)| {
            let pad = key_w.saturating_sub(k.width());
            Line::from(vec![
                Span::styled((*k).to_string(), key_style),
                Span::raw(" ".repeat(pad)),
                Span::raw("  "),
                Span::styled((*d).to_string(), desc_style),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_keys_include_mode_toggle() {
        assert!(
            HELP_KEYS
                .iter()
                .any(|(k, _)| k.contains('[') && k.contains(']')),
            "help popup must document the mode-toggle binding"
        );
    }

    #[test]
    fn handle_key_help_closes_on_question_mark() {
        let mut visible = true;
        let cmd = handle_key_help(&mut visible, KeyCode::Char('?'));
        assert_eq!(cmd, AppCommand::Continue);
        assert!(!visible);
    }

    #[test]
    fn handle_key_help_closes_on_q_and_esc_without_quitting() {
        for key in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut visible = true;
            let cmd = handle_key_help(&mut visible, key);
            assert_eq!(cmd, AppCommand::Continue);
            assert!(!visible);
        }
    }

    #[test]
    fn handle_key_help_swallows_other_keys() {
        let mut visible = true;
        handle_key_help(&mut visible, KeyCode::Char('j'));
        assert!(visible, "unrelated keys must not close the popup");
    }
}
