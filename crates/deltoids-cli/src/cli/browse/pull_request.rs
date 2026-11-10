//! The mode `deltoids pr` drives: one pull request under review, laid
//! out as the workspace lays out a commit review, with the file tree on
//! the left and the diff in the main view.
//!
//! The pull request resolves and builds on the panel's worker thread, so
//! the TUI opens at once and shows `Loading…`, or the error that stopped
//! it. Nothing is watched: a pull request is a fixed pair of commits.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

use deltoids::Theme;

use super::files::{Snapshot, SnapshotPanel, pull_request_label};
use super::mode::{AppCommand, BackgroundWork, DrawBudget, Mode, Viewport};
use super::pane;
use super::panel::{Panel, panel_title};
use super::watch::ChangeReceiver;
use crate::pull_request::{Opener, PrRef};

pub(super) struct PullRequestMode {
    panel: SnapshotPanel,
    main_focused: bool,
    side_rect: Rect,
    main_rect: Rect,
}

impl PullRequestMode {
    /// Review `pr`, from the clone that contains `cwd` when there is one.
    pub(super) fn open(pr: PrRef, cwd: PathBuf) -> Self {
        let name = pr.to_string();
        let label = pull_request_label(&name);
        let panel = SnapshotPanel::resolving(
            label,
            Box::new(move |shown| {
                let opened = Opener::from_env().open(&pr, &cwd, shown)?;
                let snapshot = Snapshot::PullRequest {
                    name,
                    info: opened.info,
                    base: opened.merge_base,
                    tip: opened.head,
                    commits: opened.commits,
                };
                Ok((opened.repo_dir, snapshot))
            }),
        );
        Self {
            panel,
            main_focused: false,
            side_rect: Rect::default(),
            main_rect: Rect::default(),
        }
    }

    fn focus(&mut self, main: bool) {
        self.main_focused = main;
        self.panel.focus_main(main);
    }

    fn focused_height(&self, main: usize) -> usize {
        if self.main_focused {
            main
        } else {
            pane::body_height(self.side_rect)
        }
    }
}

impl Mode for PullRequestMode {
    fn background(&mut self, active: bool) -> BackgroundWork {
        self.panel.background(active)
    }

    fn build(&mut self, _theme: &Theme, _diff_width: usize) {}

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
        let title = panel_title(&self.panel.label());
        self.panel
            .draw_side(frame, left, title, !self.main_focused, theme);
        self.panel
            .draw_main(frame, right, self.main_focused, theme, budget);
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        let height = self.focused_height(height);
        let command = self.panel.handle_key_event(key, height);
        self.main_focused = self.panel.main_focused();
        command
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let height = self.focused_height(height);
        if !self.captures_text_input() && key == KeyCode::Esc {
            if !self.panel.escape() && self.main_focused {
                self.focus(false);
            }
            return AppCommand::Continue;
        }
        let command = self.panel.handle_key(key, height);
        self.main_focused = self.panel.main_focused();
        command
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.panel.reserves_key(key)
    }

    fn captures_text_input(&self) -> bool {
        self.panel.captures_text_input()
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        self.panel.report_copy(result);
    }

    /// A click focuses the half under the pointer; the wheel scrolls
    /// whatever is under the pointer without moving focus.
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
        self.panel.handle_mouse(mouse, panel_height)
    }

    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String> {
        Ok(None)
    }

    fn should_reload(&self, _paths: &[PathBuf]) -> bool {
        false
    }

    fn reload(&mut self, _viewport: Viewport, _theme: &Theme) -> Result<bool, String> {
        Ok(false)
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.panel.selected_path()
    }
}
