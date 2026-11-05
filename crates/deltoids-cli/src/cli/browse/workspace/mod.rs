//! The root mode the shell drives: lazygit's layout with one side panel.
//!
//! The left column shows the Status panel ([`StatusPanel`]): the
//! uncommitted changes above the branches and their commits, drawn like
//! `but status`. The right pane is the main view, showing what the
//! selection drives. While a review of a commit or a branch is open, the
//! review takes the left column ([`SnapshotPanel`]); `Esc` closes it and
//! returns to the Status panel, still on the reviewed row.
//!
//! Focus is on the side panel or on the main view. `Enter` on a file
//! focuses its diff, a click focuses the half under the pointer, and
//! `Esc` leaves the main view. Every other key goes to the shown panel,
//! which owns both of its halves through the [`Panel`] seam.
//!
//! The working tree is the one watched source. A reload rebuilds its
//! diff and re-reads the branches. Changes that appear on a clean tree
//! take the cursor and focus.

use std::ffi::OsString;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

use deltoids::Theme;

use super::files::{FilesMode, SnapshotPanel};
use super::history::History;
use super::mode::{AppCommand, BackgroundWork, DrawBudget, Mode, Viewport};
use super::pane;
use super::panel::{Panel, panel_title};
use super::status::StatusPanel;
use super::vcs::Vcs;
use super::watch::ChangeReceiver;

pub(super) struct Workspace {
    status: StatusPanel,
    /// A review shown in the left column in place of `status`.
    review: Option<SnapshotPanel>,
    main_focused: bool,
    side_rect: Rect,
    main_rect: Rect,
    /// The `but` program the branches run, when not the one on `PATH`.
    but: Option<OsString>,
}

impl Workspace {
    pub(super) fn new(files: FilesMode) -> Self {
        Self::with_but(files, None)
    }

    fn with_but(files: FilesMode, but: Option<OsString>) -> Self {
        let branches = open_branches(&files, but.clone());
        Self {
            status: StatusPanel::new(files, branches),
            review: None,
            main_focused: false,
            side_rect: Rect::default(),
            main_rect: Rect::default(),
            but,
        }
    }

    fn panel(&mut self) -> &mut dyn Panel {
        match &mut self.review {
            Some(review) => review,
            None => &mut self.status,
        }
    }

    fn panel_ref(&self) -> &dyn Panel {
        match &self.review {
            Some(review) => review,
            None => &self.status,
        }
    }

    /// Replace the startup placeholder with the built working tree.
    fn adopt_files(&mut self, files: FilesMode) {
        let branches = open_branches(&files, self.but.clone());
        self.status.replace(files, branches);
        self.focus(false);
    }

    /// Give focus to the shown panel's side (`false`) or its main view.
    fn focus(&mut self, main: bool) {
        self.main_focused = main;
        self.panel().focus_main(main);
    }

    /// Hand a key to the shown panel, then follow any focus move it
    /// made on its own.
    fn forward_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let command = self.panel().handle_key(key, height);
        self.main_focused = self.panel().main_focused();
        self.open_review();
        command
    }

    /// Show the review the branches asked for and focus it. The review
    /// already open stays, comments and all, when it shows the same
    /// snapshot.
    fn open_review(&mut self) {
        let Some(snapshot) = self.status.take_review_request() else {
            return;
        };
        if !self.review.as_ref().is_some_and(|r| r.shows(&snapshot)) {
            let changes = self.status.changes();
            let Some(dir) = changes.repo_dir() else {
                return;
            };
            let review = SnapshotPanel::review(dir, snapshot).with_servers(changes.servers());
            self.review = Some(review);
        }
        self.focus(false);
    }

    /// Inner height of the half with focus; `main` is the main view's.
    fn focused_height(&self, main: usize) -> usize {
        if self.main_focused {
            main
        } else {
            pane::body_height(self.side_rect)
        }
    }
}

/// The branches of the working tree's repository; `None` outside one.
fn open_branches(files: &FilesMode, but: Option<OsString>) -> Option<History> {
    let servers = files.servers();
    files
        .repo_dir()
        .and_then(|dir| Vcs::discover_at(&dir))
        .map(|vcs| match but {
            Some(program) => vcs.with_but(program),
            None => vcs,
        })
        .map(|vcs| History::branches(vcs).with_servers(servers))
}

impl Mode for Workspace {
    fn background(&mut self, active: bool) -> BackgroundWork {
        // Unfocused or behind a review, edits still reach the servers.
        self.status.changes().poll_servers();
        let mut work = self.status.background(active);
        if let Some(review) = self.review.as_mut() {
            let more = review.background(active);
            work.changed |= more.changed;
            work.pending |= more.pending;
        }
        work
    }

    fn build(&mut self, theme: &Theme, diff_width: usize) {
        self.adopt_files(FilesMode::discover(theme, diff_width));
    }

    fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        left: Rect,
        right: Rect,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        self.side_rect = left;
        self.main_rect = right;
        let main_focused = self.main_focused;
        let panel = self.panel();
        let title = panel_title(&panel.label());
        panel.draw_side(frame, left, title, !main_focused, theme);
        panel.draw_main(frame, right, main_focused, theme, budget);
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.handle_key(key.code, height);
        }
        let height = self.focused_height(height);
        let command = self.panel().handle_key_event(key, height);
        self.main_focused = self.panel().main_focused();
        command
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let height = self.focused_height(height);
        if self.captures_text_input() {
            return self.forward_key(key, height);
        }
        match key {
            KeyCode::Esc if self.panel().escape() => {}
            KeyCode::Esc if self.main_focused => self.focus(false),
            KeyCode::Esc if self.review.is_some() => {
                self.review = None;
                self.focus(false);
            }
            _ => return self.forward_key(key, height),
        }
        AppCommand::Continue
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.panel_ref().reserves_key(key)
    }

    fn take_refresh_request(&mut self) -> bool {
        self.status.changes_mut().take_refresh_request()
    }

    fn captures_text_input(&self) -> bool {
        self.panel_ref().captures_text_input()
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        self.panel().report_copy(result);
    }

    /// A click focuses the half under the pointer; the event then goes
    /// to the shown panel. The wheel scrolls whatever is under the
    /// pointer without moving focus.
    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        if self.captures_text_input() {
            return AppCommand::Continue;
        }
        let position = Position::new(mouse.column, mouse.row);
        let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        let panel_height = if self.main_rect.contains(position) {
            if click {
                self.focus(true);
            }
            height
        } else if self.side_rect.contains(position) {
            if click {
                self.focus(false);
            }
            pane::body_height(self.side_rect)
        } else if mouse.kind == MouseEventKind::Moved {
            height
        } else {
            return AppCommand::Continue;
        };
        let command = self.panel().handle_mouse(mouse, panel_height);
        self.open_review();
        command
    }

    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String> {
        self.status.changes_mut().watch()
    }

    fn should_reload(&self, paths: &[PathBuf]) -> bool {
        self.status.changes().should_reload(paths)
    }

    fn retry_reload(&self) -> bool {
        self.status.changes().retry_reload()
    }

    /// Rebuild the working tree's diff and re-read the branches. Commits
    /// and branches move with the working tree (a commit empties the
    /// index), so this always asks for a frame. Changes on a clean tree
    /// take the cursor and focus.
    fn reload(&mut self, viewport: Viewport, theme: &Theme) -> Result<bool, String> {
        let was_clean = self.status.changes().is_clean();
        let changed = self.status.changes_mut().reload(viewport, theme)?;
        let branches = match self.status.branches_mut() {
            Some(branches) => {
                branches.refresh();
                true
            }
            None => false,
        };
        if self.review.is_none() && was_clean && !self.status.changes().is_clean() {
            self.status.focus_changes();
            self.main_focused = false;
        } else {
            self.status.settle();
            self.main_focused = self.panel().main_focused();
        }
        Ok(changed || branches)
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.panel_ref().selected_path()
    }
}

#[cfg(test)]
mod tests;
