//! The `Panel` seam: one side panel of the lazygit layout, plus
//! the main view it drives while it is the current panel.
//!
//! [`super::workspace::Workspace`] shows the current side panel in the
//! left column, titled with a strip of every side panel's tab
//! ([`tab_strip`]), gives it the main view on the right, and decides
//! which half has focus. A panel owns everything else: its state, its
//! keys, its mouse hit-testing, and what both halves render.

use std::ops::Range;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use super::mode::{AppCommand, BackgroundWork, DrawBudget};
use crate::sidebar::{IconMode, filter_icon};

pub(super) trait Panel {
    /// The side panel's name, as its tab reads.
    fn label(&self) -> String;

    /// Draw the side panel into `area` under `title`. `focused` when
    /// the side panel has key focus.
    fn draw_side(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        title: Line<'static>,
        focused: bool,
        theme: &Theme,
    );

    /// Draw the main view this panel drives into `area`. `focused` when
    /// the main view has key focus.
    fn draw_main(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        budget: DrawBudget,
    );

    /// Send keys to the main view (`true`) or to the side panel.
    fn focus_main(&mut self, main: bool);

    /// Whether keys go to the main view. A panel may move focus there on
    /// its own, as `Enter` on a file does.
    fn main_focused(&self) -> bool;

    /// Handle a key for whichever half has focus. `height` is that
    /// half's inner height.
    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand;

    /// Handle a key with its modifiers, as
    /// [`super::mode::Mode::handle_key_event`].
    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        self.handle_key(key.code, height)
    }

    /// Drop transient state of the panel's own on `Esc` (a pending
    /// lookup, an open list) and say whether there was any. When there
    /// was none, `Esc` keeps its workspace meaning.
    fn escape(&mut self) -> bool {
        false
    }

    /// Handle a mouse event over either half, hit-tested against the
    /// rects of the last draw. `height` is the inner height of the half
    /// under the pointer.
    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand;

    /// Collect background render work, as [`super::mode::Mode::background`].
    fn background(&mut self, _active: bool) -> BackgroundWork {
        BackgroundWork::default()
    }

    /// Keys this panel binds over configured custom commands.
    fn reserves_key(&self, _key: KeyCode) -> bool {
        false
    }

    /// Whether a modal editor owns every key.
    fn captures_text_input(&self) -> bool {
        false
    }

    /// The outcome of a clipboard copy this panel asked for.
    fn report_copy(&mut self, _result: Result<(), String>) {}

    /// The file on disk the selection points at, for custom commands.
    fn selected_path(&self) -> Option<PathBuf> {
        None
    }
}

/// A pane's top border title: `─Files─`. The label uses the bold
/// accent; the `─` rules use
/// `rule_color`, the pane's own border colour, so the title stays
/// continuous with the border (accent when focused, plain otherwise).
pub(crate) fn panel_title(rule_color: Color, label: &str, theme: &Theme) -> Line<'static> {
    let rule = Style::default().fg(rule_color);
    let accent = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled("─", rule),
        Span::styled(label.to_string(), accent),
        Span::styled("─", rule),
    ])
}

/// The side panels' tabs as one top border title:
/// `─Files─Local branches─`. The `current` tab reads in the bold accent,
/// the others muted; the `─` rules use `rule_color`, as in
/// [`panel_title`]. Also returns each tab's columns, counted from the
/// title's first cell.
pub(crate) fn tab_strip(
    rule_color: Color,
    tabs: &[String],
    current: usize,
    theme: &Theme,
) -> (Line<'static>, Vec<Range<u16>>) {
    let rule = Style::default().fg(rule_color);
    let accent = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let mut spans = vec![Span::styled("─", rule)];
    let mut ranges = Vec::with_capacity(tabs.len());
    let mut column: u16 = 1;
    for (index, label) in tabs.iter().enumerate() {
        let style = if index == current { accent } else { muted };
        let start = column;
        column += label.width() as u16;
        ranges.push(start..column);
        column += 1;
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::styled("─", rule));
    }
    (Line::from(spans), ranges)
}

/// Paint the filter glyph at the right end of `area`'s top border, one
/// rule in from the corner, to say a filter hides some changes.
pub(crate) fn draw_filter_flag(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let glyph = filter_icon(IconMode::from_env());
    let width = glyph.width() as u16;
    if area.width < width + 4 || area.height == 0 {
        return;
    }
    let flag = Rect {
        x: area.right() - 2 - width,
        y: area.y,
        width,
        height: 1,
    };
    let style = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(Span::styled(glyph, style)), flag);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_reads_its_label_in_the_accent_between_border_rules() {
        let theme = Theme::default();
        let rule_color = rgb_to_color(theme.border);
        let line = panel_title(rule_color, "Files", &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "─Files─");
        let files = line.spans.iter().find(|s| s.content == "Files").unwrap();
        assert_eq!(files.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(files.style.add_modifier.contains(Modifier::BOLD));
        let rule = line.spans.iter().find(|s| s.content == "─").unwrap();
        assert_eq!(rule.style.fg, Some(rule_color));
    }

    #[test]
    fn the_tab_strip_reads_every_tab_and_accents_the_current_one() {
        let theme = Theme::default();
        let rule_color = rgb_to_color(theme.border);
        let tabs = ["Files".to_string(), "Local branches".to_string()];
        let (line, ranges) = tab_strip(rule_color, &tabs, 1, &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "─Files─Local branches─");
        let style = |content: &str| {
            line.spans
                .iter()
                .find(|s| s.content == content)
                .unwrap()
                .style
        };
        assert_eq!(style("Files").fg, Some(rgb_to_color(theme.muted)));
        assert_eq!(
            style("Local branches").fg,
            Some(rgb_to_color(theme.border_active))
        );
        assert_eq!(style("─").fg, Some(rule_color));
        let chars: Vec<char> = text.chars().collect();
        let tab = |range: &Range<u16>| -> String {
            chars[range.start as usize..range.end as usize]
                .iter()
                .collect()
        };
        assert_eq!(tab(&ranges[0]), "Files");
        assert_eq!(tab(&ranges[1]), "Local branches");
    }
}
