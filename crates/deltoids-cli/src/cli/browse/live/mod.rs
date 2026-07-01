//! Live mode: an ephemeral, in-memory feed of working-tree edits as they
//! happen.
//!
//! Unlike Traces (agent intent, persisted under the trace root), Live
//! observes the filesystem directly: every time a watched file changes on
//! disk it appends one feed entry diffing the file against its
//! last-known state. It needs no plugin or agent integration and works
//! for any tool that writes files. The feed lives only while the tab is
//! open; nothing is persisted.
//!
//! Placeholder: the engine ([`model`]) and the full pane adapter land in
//! later phases. For now the mode renders an empty state so the shell can
//! cycle to it.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, MouseEvent};
use ratatui::layout::Rect;

use deltoids::Theme;

use super::mode::{AppCommand, Mode, ReloadViewport, TabStrip};

/// Live-mode state. Placeholder until the engine and panes land.
pub(super) struct LiveMode {}

impl LiveMode {
    /// A cheap empty Live mode. Used as the startup placeholder and the
    /// degraded fallback.
    pub(super) fn empty() -> Self {
        Self {}
    }

    /// Build the Live mode from the discovered repo. Placeholder.
    pub(super) fn build() -> Result<Self, String> {
        Ok(Self::empty())
    }
}

impl Mode for LiveMode {
    fn draw(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        left: Rect,
        right: Rect,
        tabs: TabStrip,
        theme: &Theme,
    ) {
        use deltoids::render_tui::{pane_block, pane_block_with_tabs, rgb_to_color};
        let border = rgb_to_color(theme.border);
        frame.render_widget(
            pane_block_with_tabs(tabs.title_line(theme), border, None),
            left,
        );
        frame.render_widget(pane_block("─Diff─", border), right);
    }

    fn handle_key(&mut self, _key: KeyCode, _lv: usize, _rv: usize) -> AppCommand {
        AppCommand::Continue
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _lv: usize, _rv: usize) -> AppCommand {
        AppCommand::Continue
    }

    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>> {
        None
    }

    fn should_reload(&self, _paths: &[PathBuf]) -> bool {
        false
    }

    fn needs_git_poll(&self) -> bool {
        false
    }

    fn reload(&mut self, _viewport: ReloadViewport, _theme: &Theme) -> Result<bool, String> {
        Ok(false)
    }
}
