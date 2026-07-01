//! The `Mode` seam: the interface the unified TUI shell drives, plus the
//! small shared values that cross it.
//!
//! The shell (`super`) is mode-agnostic: it owns the terminal, the event
//! loop, the one draggable divider, sidebar sizing, the help popup, and
//! mode switching (`[`/`]` cycling and tab clicks, via [`TabStrip::hit_test`]).
//! Everything that genuinely varies between the
//! left-panel modes lives behind this trait. Two adapters implement it:
//! [`super::files::FilesMode`] (the working-tree / piped-diff view) and
//! [`super::traces::TracesMode`] (the edit/write trace browser).
//!
//! Each mode owns its full vertical slice: state, key handling, mouse
//! hit-testing, render, and live-reload. The shell never reaches inside.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

/// The result of handling one input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppCommand {
    Continue,
    Quit,
}

/// How much work a mode may spend on this frame.
///
/// The shell passes `Fast` while input is actively streaming (the user is
/// holding a navigation key) and `Full` once input settles. A mode uses
/// this to defer expensive, deferrable rendering (Traces mode skips
/// building an entry's syntax-highlighted diff on `Fast` frames and shows
/// a cheap placeholder instead). Modes whose draw is already cheap ignore
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawBudget {
    Full,
    Fast,
}

/// Whether to build (highlight) a deferrable diff body this frame. Always
/// build on a `Full` frame; on a `Fast` frame (input streaming) build only
/// when it is already cached, so navigation never blocks on highlighting an
/// unseen large item. Shared by both mode adapters (Files keys the cache by
/// file index, Traces by `(trace, entry)`).
pub(crate) fn should_build_body(budget: DrawBudget, already_cached: bool) -> bool {
    budget == DrawBudget::Full || already_cached
}

/// Labels for the modes, in `active`-index order.
pub(crate) const TAB_LABELS: [&str; 2] = ["Files", "Traces"];

/// Which mode is active, handed to the active mode at draw time so it can
/// render the `Files - Traces` tab strip in its top-left panel
/// title.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TabStrip {
    pub(crate) active: usize,
}

/// One piece of the tab strip, in left-to-right order. `text` is the
/// literal glyphs; `mode` is `Some(index)` for a clickable label,
/// `None` for the surrounding rules and separators.
struct StripPiece {
    text: &'static str,
    mode: Option<usize>,
}

/// The tab strip's layout: a prefix rule + `[1]` badge, then each label
/// separated by `" - "`, then a trailing rule. Painting (`title_line`)
/// and hit-testing (`hit_test`) both walk this so the geometry lives in
/// one place.
fn strip_pieces() -> Vec<StripPiece> {
    let mut pieces = vec![
        StripPiece {
            text: "─",
            mode: None,
        },
        StripPiece {
            text: "[1]",
            mode: None,
        },
        StripPiece {
            text: "─",
            mode: None,
        },
    ];
    for (index, label) in TAB_LABELS.iter().enumerate() {
        if index > 0 {
            pieces.push(StripPiece {
                text: " - ",
                mode: None,
            });
        }
        pieces.push(StripPiece {
            text: label,
            mode: Some(index),
        });
    }
    pieces.push(StripPiece {
        text: "─",
        mode: None,
    });
    pieces
}

impl TabStrip {
    /// Build the styled title line for the top-left panel in lazygit's
    /// style: `─[1]─Files - Traces─`. The pane badge `[1]` and the active
    /// label use the bold accent; inactive labels and separators use the
    /// terminal's default foreground (white on dark themes), matching
    /// lazygit; the surrounding `─` rules use `rule_color`, which callers
    /// set to the pane's own block-border colour so the strip stays
    /// continuous with the border (accent when focused, plain otherwise).
    pub(crate) fn title_line(self, rule_color: Color, theme: &Theme) -> Line<'static> {
        let border = Style::default().fg(rule_color);
        let active = Style::default()
            .fg(rgb_to_color(theme.border_active))
            .add_modifier(Modifier::BOLD);
        let inactive = Style::default().fg(Color::Reset);

        let spans = strip_pieces()
            .into_iter()
            .map(|piece| {
                let style = match piece.mode {
                    // `[1]` badge and the surrounding rules.
                    None if piece.text == "[1]" => active,
                    None if piece.text == "─" => border,
                    // Separators.
                    None => inactive,
                    // Labels: active bold-accent, others default fg.
                    Some(index) if index == self.active => active,
                    Some(_) => inactive,
                };
                Span::styled(piece.text.to_string(), style)
            })
            .collect::<Vec<_>>();
        Line::from(spans)
    }

    /// Map a screen column to the mode index whose label sits under it, or
    /// `None` when the column falls on the prefix, a separator, the
    /// trailing rule, or outside the strip entirely. `title_start_x` is
    /// the screen column of the strip's first glyph (one column in from
    /// the panel's left border).
    pub(crate) fn hit_test(self, col: u16, title_start_x: u16) -> Option<usize> {
        let mut x = title_start_x;
        for piece in strip_pieces() {
            let width = piece.text.chars().count() as u16;
            if let Some(index) = piece.mode
                && col >= x
                && col < x + width
            {
                return Some(index);
            }
            x += width;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_body_full_always_builds_fast_only_when_cached() {
        assert!(should_build_body(DrawBudget::Full, false));
        assert!(should_build_body(DrawBudget::Full, true));
        assert!(!should_build_body(DrawBudget::Fast, false));
        assert!(should_build_body(DrawBudget::Fast, true));
    }

    #[test]
    fn hit_test_maps_columns_to_mode_indices() {
        // Layout from title_start_x = 1: prefix `─[1]─` = cols 1..6,
        // then Files 6..11, " - " 11..14, Traces 14..20, trailing `─` 20.
        let strip = TabStrip { active: 0 };
        let start = 1;
        // Prefix columns miss.
        assert_eq!(strip.hit_test(1, start), None);
        assert_eq!(strip.hit_test(5, start), None);
        // Files label.
        assert_eq!(strip.hit_test(6, start), Some(0));
        assert_eq!(strip.hit_test(10, start), Some(0));
        // Separator misses.
        assert_eq!(strip.hit_test(11, start), None);
        // Traces label.
        assert_eq!(strip.hit_test(14, start), Some(1));
        assert_eq!(strip.hit_test(19, start), Some(1));
        // Past the strip misses.
        assert_eq!(strip.hit_test(20, start), None);
        assert_eq!(strip.hit_test(100, start), None);
        // Before the strip misses.
        assert_eq!(strip.hit_test(0, start), None);
    }

    #[test]
    fn tab_strip_highlights_active_label() {
        let theme = Theme::default();
        let rule_color = rgb_to_color(theme.border_active);
        let line = TabStrip { active: 1 }.title_line(rule_color, &theme);
        // Spans: ─ [1] ─ Files " - " Traces ─
        let find = |c: &str| line.spans.iter().find(|s| s.content == c).unwrap();
        let badge = find("[1]");
        let files = find("Files");
        let traces = find("Traces");
        // The pane badge is always the bold accent.
        assert_eq!(badge.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(badge.style.add_modifier.contains(Modifier::BOLD));
        // Active (Traces) is the bold accent; inactive (Files) uses the
        // terminal default foreground (white on dark themes).
        assert_eq!(traces.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(traces.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(files.style.fg, Some(Color::Reset));
    }

    #[test]
    fn tab_strip_rules_follow_passed_border_color() {
        let theme = Theme::default();
        // A focused pane hands its accent border colour in; the connecting
        // `─` rules must match it, not the default (blue) theme border.
        let rule_color = rgb_to_color(theme.border_active);
        let line = TabStrip { active: 0 }.title_line(rule_color, &theme);
        let rule = line.spans.iter().find(|s| s.content == "─").unwrap();
        assert_eq!(rule.style.fg, Some(rule_color));
        assert_ne!(rule.style.fg, Some(rgb_to_color(theme.border)));

        // An unfocused pane hands the plain border colour; rules track it.
        let rule_color = rgb_to_color(theme.border);
        let line = TabStrip { active: 0 }.title_line(rule_color, &theme);
        let rule = line.spans.iter().find(|s| s.content == "─").unwrap();
        assert_eq!(rule.style.fg, Some(rule_color));
    }
}

/// Viewport sizes a mode needs to reload in place: the inner heights of
/// the left column and right pane, plus the right pane's inner width.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReloadViewport {
    pub(crate) left_viewport: usize,
    pub(crate) right_viewport: usize,
    pub(crate) right_width: usize,
}

/// A cyclable left-panel mode. The shell holds one boxed adapter per
/// mode and cycles between them; each adapter owns its own selection,
/// scroll, focus, and reload machinery.
pub(crate) trait Mode {
    /// Render the left column into `left` and the diff/detail into
    /// `right`. The mode subdivides `left` itself (Files: one panel;
    /// Traces: two stacked panels) and caches its sub-rects for mouse
    /// hit-testing. `tabs` carries the active-mode index so the mode
    /// draws the tab strip in its top-left panel title. `budget` tells
    /// the mode whether it may defer expensive rendering this frame
    /// (`Fast` while input streams, `Full` once it settles).
    fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        left: Rect,
        right: Rect,
        tabs: TabStrip,
        theme: &Theme,
        budget: DrawBudget,
    );

    /// Handle a key already stripped of the shell's global bindings
    /// (quit, help, mode toggle, sidebar resize).
    fn handle_key(
        &mut self,
        key: KeyCode,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand;

    /// Handle a mouse event already filtered of divider-drag handling.
    /// The mode hit-tests within the left column / right pane using the
    /// rects it cached at draw time.
    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand;

    /// Arm the change-notification watcher for this mode's data source
    /// and return its receiver, or `None` for a static source. Called
    /// once at startup; the mode keeps the watcher handle alive and the
    /// shell drains the receiver each loop.
    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>>;

    /// Whether a batch of changed paths warrants a reload of this mode.
    fn should_reload(&self, paths: &[PathBuf]) -> bool;

    /// Whether the shell should run its periodic git poll for this mode
    /// (Files mode watching a working tree returns `true`).
    fn needs_git_poll(&self) -> bool;

    /// Reload from disk in place, preserving navigation state. Returns
    /// `true` when the visible content actually changed.
    fn reload(&mut self, viewport: ReloadViewport, theme: &Theme) -> Result<bool, String>;
}
