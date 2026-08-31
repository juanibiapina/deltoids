//! Live syntax-theme picker popup.
//!
//! A centered, bordered overlay (same chrome as the help popup) listing
//! every registry theme from [`deltoids::theme_names`]. The shell owns
//! visibility and drives this slice; picking a theme bubbles up an
//! [`AppCommand::SetSyntaxTheme`] so the `run()` loop can update the shared
//! [`Theme`], whose name joins every diff cache's epoch and triggers a live
//! re-highlight without re-parsing.

use crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

/// Open picker state: the registry names and the highlighted cursor row.
pub(super) struct ThemePicker {
    names: &'static [&'static str],
    selected: usize,
}

/// What the shell should do after a key while the picker is open.
pub(super) enum PickerAction {
    /// Keep the popup open (navigation or a swallowed key).
    Stay,
    /// Close without changing the theme.
    Close,
    /// Close and apply this registry theme name.
    Pick(&'static str),
}

impl ThemePicker {
    /// Open the picker with `current` highlighted, or the first row when the
    /// name is not in the registry.
    pub(super) fn open(current: &str) -> Self {
        let names = deltoids::theme_names();
        let selected = names.iter().position(|n| *n == current).unwrap_or(0);
        Self { names, selected }
    }

    /// The `&'static` name under the cursor, or `""` when the registry is
    /// somehow empty.
    fn selected_name(&self) -> &'static str {
        self.names.get(self.selected).copied().unwrap_or("")
    }

    /// Handle one key. `j`/`k`/arrows move; `g`/`G` jump; Enter applies;
    /// Esc/`t` close; every other key is swallowed so the popup is modal.
    pub(super) fn handle_key(&mut self, key: KeyCode) -> PickerAction {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                PickerAction::Stay
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                PickerAction::Stay
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.selected = 0;
                PickerAction::Stay
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.names.len().saturating_sub(1);
                PickerAction::Stay
            }
            KeyCode::Enter => PickerAction::Pick(self.selected_name()),
            KeyCode::Esc | KeyCode::Char('t') => PickerAction::Close,
            _ => PickerAction::Stay,
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.names.is_empty() {
            return;
        }
        let len = self.names.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    /// The inclusive-start index of the visible window that keeps the
    /// selected row on screen given `visible` rows.
    fn window_start(&self, visible: usize) -> usize {
        if visible == 0 || self.selected < visible {
            0
        } else {
            self.selected + 1 - visible
        }
    }
}

/// Render the picker as a centered, bordered overlay. The cursor row uses
/// the bold accent; the currently-applied theme (`active_name`) carries a
/// `●` marker so it stays identifiable even as the cursor moves.
pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: &Theme,
    picker: &ThemePicker,
    active_name: &str,
) {
    let longest = picker.names.iter().map(|n| n.width()).max().unwrap_or(10);
    // 2 marker cols + name; + 4 for borders + horizontal padding.
    let want_w = (longest as u16).saturating_add(2 + 4);
    let want_h = (picker.names.len() as u16).saturating_add(2);
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

    let visible = h.saturating_sub(2) as usize;
    let start = picker.window_start(visible);
    let cursor_style = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    let name_style = Style::default().fg(rgb_to_color(theme.muted));

    let rows: Vec<Line<'static>> = picker
        .names
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, name)| {
            let marker = if *name == active_name { "● " } else { "  " };
            let style = if i == picker.selected {
                cursor_style
            } else {
                name_style
            };
            Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::styled((*name).to_string(), style),
            ])
        })
        .collect();

    frame.render_widget(Clear, popup);
    let block = pane_block("─Syntax theme─", pane_border_color(true, theme));
    let inner = block.inner(popup).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(rows), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_highlights_current_theme() {
        let picker = ThemePicker::open("GitHub");
        assert_eq!(picker.selected_name(), "GitHub");
    }

    #[test]
    fn open_unknown_falls_back_to_first_row() {
        let picker = ThemePicker::open("no-such-theme");
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn navigation_wraps_and_enter_picks() {
        let mut picker = ThemePicker::open(deltoids::theme_names()[0]);
        assert!(matches!(
            picker.handle_key(KeyCode::Char('k')),
            PickerAction::Stay
        ));
        // Wrapped from the first row to the last.
        assert_eq!(picker.selected, deltoids::theme_names().len() - 1);
        let picked = picker.handle_key(KeyCode::Enter);
        match picked {
            PickerAction::Pick(name) => {
                assert_eq!(name, *deltoids::theme_names().last().unwrap())
            }
            _ => panic!("Enter should pick the cursor row"),
        }
    }

    #[test]
    fn esc_and_t_close_without_picking() {
        let mut picker = ThemePicker::open("GitHub");
        assert!(matches!(
            picker.handle_key(KeyCode::Esc),
            PickerAction::Close
        ));
        assert!(matches!(
            picker.handle_key(KeyCode::Char('t')),
            PickerAction::Close
        ));
    }

    #[test]
    fn window_start_keeps_selection_visible() {
        let mut picker = ThemePicker::open(deltoids::theme_names()[0]);
        picker.selected = 20;
        // With 5 visible rows, the window ends at the selection.
        assert_eq!(picker.window_start(5), 16);
        picker.selected = 2;
        assert_eq!(picker.window_start(5), 0);
    }
}
