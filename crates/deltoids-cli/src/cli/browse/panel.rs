//! The `Panel` seam: the side panel of the lazygit layout, plus
//! the main view it drives.
//!
//! [`super::workspace::Workspace`] shows one panel in the left column,
//! the Status panel or a review, gives it the main view on the right,
//! and decides which half has focus. A panel owns everything else: its state, its
//! keys, its mouse hit-testing, and what both halves render.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use super::mode::{AppCommand, BackgroundWork, DrawBudget};
use crate::sidebar::{IconMode, filter_icon};

pub(super) trait Panel {
    /// The side panel's name, as its title reads.
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

/// A pane's title: its label in bold. The pane draws it in the focus
/// colour.
pub(crate) fn panel_title(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// The filter glyph, a badge saying a filter hides some changes.
pub(crate) fn filter_flag(theme: &Theme) -> Span<'static> {
    Span::styled(
        filter_icon(IconMode::from_env()),
        Style::default()
            .fg(rgb_to_color(theme.border_active))
            .add_modifier(Modifier::BOLD),
    )
}
