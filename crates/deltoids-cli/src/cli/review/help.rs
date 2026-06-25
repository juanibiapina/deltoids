//! Help popup slice: the key table that is the source of truth for
//! bindings, the popup's own key handling, and its centered overlay
//! render.

use crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

use super::{AppCommand, ViewState};

/// Single source of truth for the help popup. Each entry is
/// `(keys, description)` and renders as one row of a two-column
/// table inside the popup.
pub(super) const HELP_KEYS: &[(&str, &str)] = &[
    ("?", "toggle this help"),
    ("Tab / 1 / 2", "focus sidebar / diff"),
    ("j / k", "move (sidebar) or scroll one line (diff)"),
    ("Shift+J / K", "scroll diff three lines (any focus)"),
    ("PgDn / PgUp", "page in current pane"),
    ("g / G", "top / bottom of current pane"),
    ("Home / End", "top / bottom of current pane"),
    ("< / >", "narrow / widen sidebar"),
    ("q / Esc", "quit (or close this popup)"),
];

/// Key dispatch while the help popup is shown. `?`, `Esc`, and `q`
/// all close the popup; everything else is swallowed. `q`/`Esc` do
/// **not** quit the app while the popup is open — closing the modal
/// first matches lazygit/k9s/vim convention.
pub(super) fn handle_key_help(state: &mut ViewState, key: KeyCode) -> AppCommand {
    match key {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
            state.help_visible = false;
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
    use crate::cli::review::model::ResolvedFile;
    use crate::cli::review::test_support::*;
    use crate::cli::review::{Focus, handle_key, handle_mouse};
    use crossterm::event::MouseEventKind;

    #[test]
    fn handle_key_question_mark_opens_help() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert!(!state.help_visible);
        handle_key(&mut state, KeyCode::Char('?'), 4, 4);
        assert!(state.help_visible);
    }

    #[test]
    fn handle_key_question_mark_toggles_help_closed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        handle_key(&mut state, KeyCode::Char('?'), 4, 4);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_esc_in_help_closes_popup_does_not_quit() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        let cmd = handle_key(&mut state, KeyCode::Esc, 4, 4);
        assert_eq!(cmd, AppCommand::Continue);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_q_in_help_closes_popup_does_not_quit() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        state.help_visible = true;
        let cmd = handle_key(&mut state, KeyCode::Char('q'), 4, 4);
        assert_eq!(cmd, AppCommand::Continue);
        assert!(!state.help_visible);
    }

    #[test]
    fn handle_key_navigation_swallowed_while_help_visible() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        state.focus = Focus::Diff;
        state.help_visible = true;
        let scroll_before = state.diff.diff_scroll;
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
        assert_eq!(state.diff.diff_scroll, scroll_before);
        assert!(state.help_visible, "unrelated keys must not close popup");
    }

    #[test]
    fn mouse_swallowed_while_help_visible() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state_with_rects(&resolved);
        state.focus = Focus::Diff;
        state.help_visible = true;
        let scroll_before = state.diff.diff_scroll;
        let focus_before = state.focus;

        let mouse = make_mouse(MouseEventKind::ScrollDown, 50, 5);
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(
            state.diff.diff_scroll, scroll_before,
            "scroll should not change while help visible"
        );
        assert_eq!(
            state.focus, focus_before,
            "focus should not change while help visible"
        );
        assert!(state.help_visible, "help should stay visible");
    }
}
