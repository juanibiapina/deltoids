//! The root mode the shell drives: lazygit's layout.
//!
//! The left column shows one of two side panels at full height, as tabs:
//! its top border lists every shown panel's tab, the current one in the
//! accent. The right pane is the main view, showing whatever the current
//! side panel drives:
//!
//! 1. Files: the working-tree file tree; main shows its diff. While a
//!    review of a commit or a branch is open, the review's files and
//!    diff instead ([`SnapshotPanel`]); `Esc` on the panel closes it.
//! 2. Branches: the local branches, or a GitButler workspace drawn like
//!    `but status`. Main shows the selected branch's log or commit's
//!    diff. Drills down into a branch's commits (main: the selected
//!    commit's diff). `Enter` on a commit, or on an applied GitButler
//!    branch, opens its review in Files.
//!
//! Files has a tab only while the working tree has changes (or is still
//! loading, or failed to build) or a review is open; otherwise Branches
//! is the only tab and keeps focus. When changes appear on a clean tree,
//! Files comes back as the current tab, focused. While a review is open it is the
//! only tab: `Esc` closes it and returns to Branches, still on the
//! reviewed row.
//!
//! Focus is on the current side panel or on the main view.
//! `Tab`/`Shift+Tab` and a click on a tab's name
//! switch side panels. `Enter` on a file focuses its diff, a click
//! focuses the half under the pointer, and `Esc` leaves the main view.
//! Every other key, `Tab` in the main view included, goes to the current
//! panel, which owns both of its halves through the [`Panel`] seam.
//!
//! The working tree is the one watched source. A reload rebuilds its
//! diff and re-reads the branches.

use std::ffi::OsString;
use std::ops::Range;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use deltoids::Theme;
use deltoids::render_tui::{pane_block_with_title_line, pane_border_color, rgb_to_color};

use super::files::{FilesMode, SnapshotPanel};
use super::history::History;
use super::mode::{AppCommand, BackgroundWork, DrawBudget, Mode, Viewport};
use super::panel::{Panel, panel_title, tab_strip};
use super::vcs::Vcs;
use super::watch::ChangeReceiver;

/// The side panels, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Files,
    Branches,
}

impl Side {
    const ALL: [Side; 2] = [Side::Files, Side::Branches];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&side| side == self)
            .expect("listed")
    }

    /// The panel `step` places down the column, wrapping around.
    fn step(self, step: isize) -> Self {
        let len = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + step).rem_euclid(len) as usize]
    }
}

pub(super) struct Workspace {
    files: FilesMode,
    /// A review shown in the Files panel in place of `files`.
    review: Option<SnapshotPanel>,
    branches: Option<History>,
    /// The side panel that drives the main view.
    side: Side,
    main_focused: bool,
    side_rect: Rect,
    /// Each shown tab's columns on the side panel's top border.
    tab_hits: Vec<(Side, Range<u16>)>,
    main_rect: Rect,
    /// The `but` program the branches panel runs, when not the one on
    /// `PATH`.
    but: Option<OsString>,
}

impl Workspace {
    pub(super) fn new(files: FilesMode) -> Self {
        Self::with_but(files, None)
    }

    fn with_but(files: FilesMode, but: Option<OsString>) -> Self {
        let mut workspace = Self {
            files,
            review: None,
            branches: None,
            side: Side::Files,
            main_focused: false,
            side_rect: Rect::default(),
            tab_hits: Vec::new(),
            main_rect: Rect::default(),
            but,
        };
        workspace.open_repository();
        workspace.settle_focus();
        workspace
    }

    /// Open the branches panel on the working tree's repository; without
    /// one it says so.
    fn open_repository(&mut self) {
        let but = self.but.clone();
        let servers = self.files.servers();
        self.branches = self
            .files
            .repo_dir()
            .and_then(|dir| Vcs::discover_at(&dir))
            .map(|vcs| match but {
                Some(program) => vcs.with_but(program),
                None => vcs,
            })
            .map(|vcs| History::branches(vcs).with_servers(servers));
    }

    fn panel(&mut self, side: Side) -> Option<&mut dyn Panel> {
        match side {
            Side::Files => Some(match &mut self.review {
                Some(review) => review as &mut dyn Panel,
                None => &mut self.files,
            }),
            Side::Branches => self.branches.as_mut().map(|h| h as &mut dyn Panel),
        }
    }

    fn panel_ref(&self, side: Side) -> Option<&dyn Panel> {
        match side {
            Side::Files => Some(match &self.review {
                Some(review) => review as &dyn Panel,
                None => &self.files,
            }),
            Side::Branches => self.branches.as_ref().map(|h| h as &dyn Panel),
        }
    }

    fn shows(&self, side: Side) -> bool {
        match side {
            Side::Files => self.review.is_some() || !self.files.is_clean(),
            Side::Branches => self.review.is_none(),
        }
    }

    /// Replace the startup placeholder with the built working tree and
    /// open on Files, or on Branches when the tree is clean.
    fn adopt_files(&mut self, files: FilesMode) {
        self.files = files;
        self.open_repository();
        self.focus(Side::Files, false);
    }

    /// Give `side` focus, on its side panel or its main view. A hidden
    /// panel hands focus to the other side panel, which then shows.
    fn focus(&mut self, side: Side, main: bool) {
        let (side, main) = if self.shows(side) {
            (side, main)
        } else {
            (side.step(1), false)
        };
        self.side = side;
        self.main_focused = main;
        if let Some(panel) = self.panel(side) {
            panel.focus_main(main);
        }
    }

    /// Hand a key to the current panel, then follow any focus move it
    /// made on its own.
    fn forward_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let side = self.side;
        let Some(panel) = self.panel(side) else {
            return AppCommand::Continue;
        };
        let command = panel.handle_key(key, height);
        self.main_focused = panel.main_focused();
        if side == Side::Branches {
            self.open_review();
        }
        command
    }

    /// Show the review the branches panel asked for in the Files panel
    /// and focus it. The review already open stays, comments and all,
    /// when it shows the same snapshot.
    fn open_review(&mut self) {
        let Some(snapshot) = self
            .branches
            .as_mut()
            .and_then(History::take_review_request)
        else {
            return;
        };
        if !self.review.as_ref().is_some_and(|r| r.shows(&snapshot)) {
            let Some(dir) = self.files.repo_dir() else {
                return;
            };
            let review = SnapshotPanel::review(dir, snapshot).with_servers(self.files.servers());
            self.set_review(Some(review));
        }
        self.focus(Side::Files, false);
    }

    /// Show `review` in the Files panel, or the working tree for `None`.
    fn set_review(&mut self, review: Option<SnapshotPanel>) {
        self.review = review;
        self.settle_focus();
    }

    /// Move focus off a panel that is no longer shown.
    fn settle_focus(&mut self) {
        if !self.shows(self.side) {
            self.focus(self.side, self.main_focused);
        }
    }

    /// Inner height of the half with focus; `main` is the main view's.
    fn focused_height(&self, main: usize) -> usize {
        if self.main_focused {
            main
        } else {
            inner_height(self.side_rect)
        }
    }

    /// The side panel whose tab sits under `position`.
    fn tab_at(&self, position: Position) -> Option<Side> {
        if position.y != self.side_rect.y || position.x + 1 >= self.side_rect.right() {
            return None;
        }
        self.tab_hits
            .iter()
            .find(|(_, columns)| columns.contains(&position.x))
            .map(|&(side, _)| side)
    }

    /// The tab strip titling the side panel, recording where each tab
    /// lands for clicks.
    fn tab_title(&mut self, area: Rect, focused: bool, theme: &Theme) -> Line<'static> {
        let shown: Vec<Side> = Side::ALL.into_iter().filter(|&s| self.shows(s)).collect();
        let tabs: Vec<String> = shown
            .iter()
            .map(|&side| {
                self.panel_ref(side)
                    .map_or_else(|| "Local branches".to_string(), Panel::label)
            })
            .collect();
        let current = shown.iter().position(|&s| s == self.side).unwrap_or(0);
        let color = pane_border_color(focused, theme);
        let (title, ranges) = tab_strip(color, &tabs, current, theme);
        let left = area.x + 1;
        self.tab_hits = shown
            .into_iter()
            .zip(ranges)
            .map(|(side, columns)| (side, left + columns.start..left + columns.end))
            .collect();
        title
    }
}

impl Mode for Workspace {
    fn background(&mut self, active: bool) -> BackgroundWork {
        // Unfocused or behind a review, edits still reach the servers.
        self.files.poll_servers();
        let mut work = BackgroundWork::default();
        for side in Side::ALL {
            if let Some(panel) = self.panel(side) {
                let panel_work = panel.background(active);
                work.changed |= panel_work.changed;
                work.pending |= panel_work.pending;
            }
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
        let (current, main_focused) = (self.side, self.main_focused);
        let title = self.tab_title(left, !main_focused, theme);
        match self.panel(current) {
            Some(panel) => panel.draw_side(frame, left, title, !main_focused, theme),
            None => draw_no_repository(frame, left, title, !main_focused, theme),
        }
        match self.panel(current) {
            Some(panel) => panel.draw_main(frame, right, main_focused, theme, budget),
            None => {
                let color = pane_border_color(main_focused, theme);
                let title = panel_title(color, "Log", theme);
                draw_no_repository(frame, right, title, main_focused, theme);
            }
        }
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.handle_key(key.code, height);
        }
        let height = self.focused_height(height);
        let side = self.side;
        let Some(panel) = self.panel(side) else {
            return AppCommand::Continue;
        };
        let command = panel.handle_key_event(key, height);
        self.main_focused = panel.main_focused();
        command
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let height = self.focused_height(height);
        if self.captures_text_input() {
            return self.forward_key(key, height);
        }
        let side = self.side;
        match key {
            KeyCode::Esc if self.panel(side).is_some_and(|panel| panel.escape()) => {}
            KeyCode::Esc if self.main_focused => self.focus(side, false),
            KeyCode::Esc if self.review.is_some() => {
                self.set_review(None);
                self.focus(Side::Branches, false);
            }
            KeyCode::Tab if !self.main_focused => {
                self.focus(side.step(1), false);
            }
            KeyCode::BackTab if !self.main_focused => {
                self.focus(side.step(-1), false);
            }
            _ => return self.forward_key(key, height),
        }
        AppCommand::Continue
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.panel_ref(self.side)
            .is_some_and(|panel| panel.reserves_key(key))
    }

    fn take_refresh_request(&mut self) -> bool {
        self.files.take_refresh_request()
    }

    fn captures_text_input(&self) -> bool {
        self.panel_ref(self.side)
            .is_some_and(|panel| panel.captures_text_input())
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        let side = self.side;
        if let Some(panel) = self.panel(side) {
            panel.report_copy(result);
        }
    }

    /// A click on a tab's name shows and focuses its side panel. Any
    /// other click focuses the half under the pointer; the event then
    /// goes to the panel that owns that half. The wheel scrolls whatever
    /// is under the pointer without moving focus.
    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        if self.captures_text_input() {
            return AppCommand::Continue;
        }
        let position = Position::new(mouse.column, mouse.row);
        let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        if click && let Some(side) = self.tab_at(position) {
            self.focus(side, false);
            return AppCommand::Continue;
        }
        let (side, panel_height) = if self.main_rect.contains(position) {
            if click {
                self.focus(self.side, true);
            }
            (self.side, height)
        } else if self.side_rect.contains(position) {
            if click {
                self.focus(self.side, false);
            }
            (self.side, inner_height(self.side_rect))
        } else {
            return AppCommand::Continue;
        };
        let command = self.panel(side).map_or(AppCommand::Continue, |panel| {
            panel.handle_mouse(mouse, panel_height)
        });
        if side == Side::Branches {
            self.open_review();
        }
        command
    }

    fn watch(&mut self) -> Result<Option<ChangeReceiver>, String> {
        self.files.watch()
    }

    fn should_reload(&self, paths: &[PathBuf]) -> bool {
        self.files.should_reload(paths)
    }

    fn retry_reload(&self) -> bool {
        self.files.retry_reload()
    }

    /// Rebuild the working tree's diff and re-read the branches. Commits
    /// and branches move with the working tree (a commit empties the
    /// index), so this always asks for a frame. Changes on a clean tree
    /// bring Files back and focus it.
    fn reload(&mut self, viewport: Viewport, theme: &Theme) -> Result<bool, String> {
        let files_hidden = !self.shows(Side::Files);
        let changed = self.files.reload(viewport, theme)?;
        if let Some(branches) = &mut self.branches {
            branches.refresh();
        }
        if files_hidden && self.shows(Side::Files) {
            self.focus(Side::Files, false);
        } else {
            self.settle_focus();
        }
        Ok(changed || self.branches.is_some())
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.panel_ref(self.side)
            .and_then(|panel| panel.selected_path())
    }
}

fn inner_height(area: Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

fn draw_no_repository(
    frame: &mut Frame<'_>,
    area: Rect,
    title: Line<'static>,
    focused: bool,
    theme: &Theme,
) {
    let color = pane_border_color(focused, theme);
    let block = pane_block_with_title_line(title, color, None);
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    frame.render_widget(
        Paragraph::new("Not a git repository.")
            .style(muted)
            .block(block),
        area,
    );
}

#[cfg(test)]
mod tests;
