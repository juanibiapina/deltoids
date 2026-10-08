//! The `Mode` seam: the interface the unified TUI shell drives, plus the
//! small shared values that cross it.
//!
//! The shell (`super`) owns the terminal, the event loop, the one
//! draggable divider, sidebar sizing, the help popup, and reload timing.
//! Everything else lives behind this trait. The production adapter is
//! [`super::files::FilesMode`] (the working-tree view); the shell tests use
//! a recording adapter.
//!
//! The mode owns its full vertical slice: state, key handling, mouse
//! hit-testing, render, and live-reload. The shell never reaches inside.

use crate::cli::browse::watch::ChangeReceiver;
use std::path::PathBuf;

use crossterm::event::{KeyCode, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;

use deltoids::{ChangeLayout, Theme};

/// A request to run a custom command, bubbled up to the `run()` loop
/// (which owns the `Terminal`). Carries the fully-expanded shell line and
/// which run path to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomRun {
    /// The expanded shell command (`{{filename}}` already substituted).
    pub(crate) command: String,
    /// `true` to suspend the TUI and run in the foreground.
    pub(crate) subprocess: bool,
}

/// The result of handling one input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppCommand {
    Continue,
    Quit,
    /// Run a custom command; handled by the `run()` loop since it may need
    /// the `Terminal`.
    Run(CustomRun),
    /// Put this text on the system clipboard; handled by the `run()` loop
    /// since it owns terminal output (the OSC 52 fallback writes there).
    /// The outcome comes back through [`Mode::report_copy`].
    CopyToClipboard(String),
    /// Apply a new syntax-theme (a `&'static` registry name); handled by the
    /// `run()` loop since it owns the shared [`Theme`]. The name joins every
    /// diff cache's epoch, so the next frame re-highlights live.
    SetSyntaxTheme(&'static str),
}

/// How much work a mode may spend on this frame.
///
/// The shell passes `Fast` while input is actively streaming (the user is
/// holding a navigation key) and `Full` once input settles. A mode uses
/// this to defer expensive, deferrable rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawBudget {
    Full,
    Fast,
}

/// Short human label for the current diff change-layout, shown in the Diff
/// pane footer so the toggle's state (and when it wraps back to `grouped`)
/// is visible.
pub(crate) fn layout_label(layout: ChangeLayout) -> String {
    match layout {
        ChangeLayout::Grouped => "grouped".to_string(),
        ChangeLayout::Interleaved { group } => match group.get() {
            1 => "interleaved".to_string(),
            n => format!("blocks of {n}"),
        },
    }
}

/// Pane sizes from the last drawn frame.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Viewport {
    /// Inner height shared by the sidebar and the diff pane.
    pub(crate) height: usize,
    /// Inner width of the diff body.
    pub(crate) diff_width: usize,
}

/// Changes and outstanding work reported by a mode's background renderer.
#[derive(Default)]
pub(crate) struct BackgroundWork {
    pub(crate) changed: bool,
    pub(crate) pending: bool,
}

/// The view the shell drives. The adapter owns its own selection, scroll,
/// focus, and reload machinery.
pub(crate) trait Mode {
    /// Collect render completions while active; pause speculative work
    /// otherwise (unfocused terminal or not yet built).
    /// `changed` requests a frame; `pending` requests a short input poll.
    fn background(&mut self, _active: bool) -> BackgroundWork {
        BackgroundWork::default()
    }

    /// Replace the startup placeholder with the real view, sized for a diff
    /// body `diff_width` columns wide.
    fn build(&mut self, theme: &Theme, diff_width: usize);

    /// Render the left column into `left` and the diff into `right`,
    /// caching the rects for mouse hit-testing. `budget` tells the mode
    /// whether it may defer expensive rendering this frame (`Fast` while
    /// input streams, `Full` once it settles).
    fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        left: Rect,
        right: Rect,
        layout: ChangeLayout,
        theme: &Theme,
        budget: DrawBudget,
    );

    /// Handle a key already stripped of the shell's global bindings
    /// (quit, help, theme picker, sidebar resize, layout toggle).
    /// `height` is the inner height of the panes.
    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand;

    /// Built-in mode keys that take priority over configured custom commands.
    fn reserves_key(&self, _key: KeyCode) -> bool {
        false
    }

    /// Consume a refresh requested by a completed mutation, including index-only writes.
    fn take_refresh_request(&mut self) -> bool {
        false
    }

    /// Whether the mode currently owns raw text input (a modal editor is
    /// open). While this is `true` the shell routes every key straight to
    /// the mode, so global bindings (`q`, `?`, `<`, `>`, `Esc`)
    /// and custom-command keys can be typed as text.
    fn captures_text_input(&self) -> bool {
        false
    }

    /// Report the outcome of an [`AppCommand::CopyToClipboard`] the shell
    /// performed, so the mode can correct its status message when the copy
    /// failed. Ignored by modes that never request a copy.
    fn report_copy(&mut self, _result: Result<(), String>) {}

    /// Handle a mouse event already filtered of divider-drag handling.
    /// The mode hit-tests within the left column / right pane using the
    /// rects it cached at draw time.
    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand;

    /// Arm the change-notification watcher for this mode's data source
    /// and return its receiver, or `None` for a static source. Called
    /// after the first build and after a backend failure. The mode keeps the
    /// watcher alive; the shell takes coalesced batches each loop.
    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String>;

    /// Whether a batch of changed paths warrants a reload of this mode.
    fn should_reload(&self, paths: &[PathBuf]) -> bool;

    /// Whether a failed read needs another attempt without a new event.
    fn retry_reload(&self) -> bool {
        false
    }

    /// Reload from disk in place, preserving navigation state. Returns
    /// `true` when the visible content actually changed.
    fn reload(&mut self, viewport: Viewport, theme: &Theme) -> Result<bool, String>;

    /// Absolute path of the file the active selection points at, or
    /// `None` when nothing selectable is on disk (empty state, no repo,
    /// directory-only selection with no file underneath). Custom
    /// commands expand their `{{filename}}` against this.
    fn selected_path(&self) -> Option<PathBuf>;
}
