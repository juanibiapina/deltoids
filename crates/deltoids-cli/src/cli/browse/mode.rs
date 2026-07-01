//! The `Mode` seam: the interface the unified TUI shell drives, plus the
//! small shared values that cross it.
//!
//! The shell (`super`) is mode-agnostic: it owns the terminal, the event
//! loop, the one draggable divider, sidebar sizing, the help popup, and
//! the `[`/`]` mode cycling. Everything that genuinely varies between the
//! left-panel modes lives behind this trait. Three adapters implement it:
//! [`super::files::FilesMode`] (the working-tree / piped-diff view),
//! [`super::traces::TracesMode`] (the edit/write trace browser), and
//! [`super::live::LiveMode`] (the ephemeral working-tree edit feed).
//!
//! Each mode owns its full vertical slice: state, key handling, mouse
//! hit-testing, render, and live-reload. The shell never reaches inside.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

/// The result of handling one input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppCommand {
    Continue,
    Quit,
}

/// Labels for the modes, in `active`-index order.
pub(crate) const TAB_LABELS: [&str; 3] = ["Files", "Traces", "Live"];

/// Which mode is active, handed to the active mode at draw time so it can
/// render the `Files - Traces - Live` tab strip in its top-left panel
/// title.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TabStrip {
    pub(crate) active: usize,
}

impl TabStrip {
    /// Build the styled title line for the top-left panel in lazygit's
    /// style: `─[1]─Files - Traces─`. The pane badge `[1]` and the active
    /// label use the bold accent; inactive labels and separators are
    /// muted; the surrounding rule is the border colour.
    pub(crate) fn title_line(self, theme: &Theme) -> Line<'static> {
        let border = Style::default().fg(rgb_to_color(theme.border));
        let active = Style::default()
            .fg(rgb_to_color(theme.border_active))
            .add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(rgb_to_color(theme.muted));

        let mut spans = vec![
            Span::styled("─", border),
            Span::styled("[1]", active),
            Span::styled("─", border),
        ];
        for (index, label) in TAB_LABELS.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" - ", muted));
            }
            let style = if index == self.active { active } else { muted };
            spans.push(Span::styled((*label).to_string(), style));
        }
        spans.push(Span::styled("─", border));
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_strip_highlights_active_label() {
        let theme = Theme::default();
        let line = TabStrip { active: 1 }.title_line(&theme);
        // Spans: ─ [1] ─ Files " - " Traces ─
        let find = |c: &str| line.spans.iter().find(|s| s.content == c).unwrap();
        let badge = find("[1]");
        let files = find("Files");
        let traces = find("Traces");
        // The pane badge is always the bold accent.
        assert_eq!(badge.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(badge.style.add_modifier.contains(Modifier::BOLD));
        // Active (Traces) is the bold accent; inactive (Files) is muted.
        assert_eq!(traces.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(traces.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(files.style.fg, Some(rgb_to_color(theme.muted)));
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
    /// draws the tab strip in its top-left panel title.
    fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        left: Rect,
        right: Rect,
        tabs: TabStrip,
        theme: &Theme,
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
