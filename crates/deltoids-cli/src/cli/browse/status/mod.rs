//! The Status panel: the one side panel, drawn like `but status`.
//!
//! The uncommitted changes come first, under `╭┄ [uncommitted]`, as the
//! working tree's file tree. The branches follow ([`History`]): each
//! stack's branches with their commits, closed by the common base, then
//! the other branches. Both sections share one pane and one cursor.
//!
//! - The body splits between the sections ([`split`]); each scrolls on
//!   its own to keep its selection in view.
//! - `j`/`k` past a section's last or first row move the cursor into the
//!   other section; `g`/`G` go to the top or bottom of the whole panel,
//!   and `Tab`/`Shift+Tab` jump to the other section.
//! - The section under the cursor owns every other key and drives the
//!   main view, through its own [`Panel`].
//! - A branch's commit list, opened with `Enter` on a branch outside the
//!   stacks, covers the whole column until `Esc`.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use super::files::{FilesMode, Snapshot};
use super::history::History;
use super::mode::{AppCommand, BackgroundWork, DrawBudget};
use super::pane;
use super::panel::Panel;

/// One list section of the Status pane, drawn without chrome of its own.
pub(super) trait Section: Panel {
    /// Rows of its list.
    fn list_rows(&self) -> usize;

    /// Draw the list into `body`, its scrollbar in column `scrollbar_x`.
    /// The selected row is highlighted only when `cursor_here`.
    fn draw_list(&mut self, frame: &mut Frame<'_>, list: ListArea, theme: &Theme);

    /// What the pane's title row shows at its right while the cursor is
    /// here.
    fn footer(&self, theme: &Theme) -> Option<Line<'static>>;

    /// Whether the list has a row to select.
    fn has_selectable(&self) -> bool;

    /// Whether no row to select lies past the selection, below it when
    /// `down`.
    fn at_edge(&self, down: bool) -> bool;

    /// Select the first row to select, or the last unless `from_top`, in
    /// a list `height` rows tall.
    fn enter(&mut self, from_top: bool, height: usize);
}

/// Where a [`Section`] draws its list.
pub(super) struct ListArea {
    pub(super) body: Rect,
    pub(super) scrollbar_x: u16,
    pub(super) cursor_here: bool,
    pub(super) focused: bool,
}

/// The section the cursor is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Changes,
    Branches,
}

/// The graph column left of the file rows.
const GUTTER: u16 = 2;

pub(super) struct StatusPanel {
    changes: FilesMode,
    branches: Option<History>,
    cursor: Part,
    /// The file rows at the last draw.
    changes_body: Rect,
    /// The branch rows at the last draw.
    branches_body: Rect,
    main_rect: Rect,
}

impl StatusPanel {
    pub(super) fn new(changes: FilesMode, branches: Option<History>) -> Self {
        let mut status = Self {
            changes,
            branches,
            cursor: Part::Changes,
            changes_body: Rect::default(),
            branches_body: Rect::default(),
            main_rect: Rect::default(),
        };
        status.settle();
        status
    }

    pub(super) fn changes(&self) -> &FilesMode {
        &self.changes
    }

    pub(super) fn changes_mut(&mut self) -> &mut FilesMode {
        &mut self.changes
    }

    pub(super) fn branches_mut(&mut self) -> Option<&mut History> {
        self.branches.as_mut()
    }

    /// Show `changes` and `branches` in place of the current ones, with
    /// the cursor on the first change, or on the first branch without
    /// changes.
    pub(super) fn replace(&mut self, changes: FilesMode, branches: Option<History>) {
        self.changes = changes;
        self.branches = branches;
        self.cursor = Part::Changes;
        self.settle();
    }

    /// The review `Enter` in the branches asked for since the last call.
    pub(super) fn take_review_request(&mut self) -> Option<Snapshot> {
        self.branches.as_mut()?.take_review_request()
    }

    /// Put the cursor on the first change, keys on the side panel.
    pub(super) fn focus_changes(&mut self) {
        self.set_cursor(Part::Changes, true);
        self.focus_main(false);
    }

    /// Move the cursor off a section with nothing to select, when the
    /// other has something.
    pub(super) fn settle(&mut self) {
        let other = self.other();
        if !self.has_selectable(self.cursor) && self.has_selectable(other) {
            self.set_cursor(other, true);
        }
    }

    fn other(&self) -> Part {
        match self.cursor {
            Part::Changes => Part::Branches,
            Part::Branches => Part::Changes,
        }
    }

    fn has_selectable(&self, part: Part) -> bool {
        self.section_ref(part).is_some_and(Section::has_selectable)
    }

    fn section(&mut self, part: Part) -> Option<&mut dyn Section> {
        match part {
            Part::Changes => Some(&mut self.changes),
            Part::Branches => self.branches.as_mut().map(|h| h as &mut dyn Section),
        }
    }

    fn section_ref(&self, part: Part) -> Option<&dyn Section> {
        match part {
            Part::Changes => Some(&self.changes),
            Part::Branches => self.branches.as_ref().map(|h| h as &dyn Section),
        }
    }

    /// The panel that owns keys and the main view: the commit list while
    /// it covers the column, else the section under the cursor.
    fn current(&mut self) -> &mut dyn Panel {
        let to_branches = self.cursor == Part::Branches || self.drilled_in();
        match &mut self.branches {
            Some(branches) if to_branches => branches,
            _ => &mut self.changes,
        }
    }

    fn current_ref(&self) -> &dyn Panel {
        match (self.cursor, &self.branches) {
            (_, Some(branches)) if branches.drilled_in() => branches,
            (Part::Branches, Some(branches)) => branches,
            _ => &self.changes,
        }
    }

    fn drilled_in(&self) -> bool {
        self.branches.as_ref().is_some_and(History::drilled_in)
    }

    /// Rows of list the section in `part` shows.
    fn list_height(&self, part: Part) -> usize {
        match part {
            Part::Changes => self.changes_body.height as usize,
            Part::Branches => self.branches_body.height as usize,
        }
    }

    /// Move the cursor into `part`, on its first row or its last.
    fn set_cursor(&mut self, part: Part, from_top: bool) {
        let height = self.list_height(part);
        if let Some(section) = self.section(part) {
            section.enter(from_top, height);
        }
        let old = self.cursor;
        self.cursor = part;
        if old != part
            && let Some(section) = self.section(old)
        {
            section.focus_main(false);
        }
    }

    /// Move the cursor into `part`, keeping that section's selection.
    fn jump(&mut self, part: Part) {
        if let Some(section) = self.section(self.cursor) {
            section.focus_main(false);
        }
        self.cursor = part;
    }

    /// Whether the cursor is in the branches.
    #[cfg(test)]
    pub(super) fn cursor_in_branches(&self) -> bool {
        self.cursor == Part::Branches
    }

    /// Keys while the side panel has focus. Moves across sections here;
    /// everything else goes to the section under the cursor.
    fn handle_side_key(&mut self, key: KeyCode) -> AppCommand {
        let other = self.other();
        let first = if self.has_selectable(Part::Changes) {
            Part::Changes
        } else {
            Part::Branches
        };
        let last = if self.has_selectable(Part::Branches) {
            Part::Branches
        } else {
            Part::Changes
        };
        match key {
            KeyCode::Tab | KeyCode::BackTab if self.has_selectable(other) => self.jump(other),
            KeyCode::Tab | KeyCode::BackTab => {}
            KeyCode::Char('g') | KeyCode::Home => self.set_cursor(first, true),
            KeyCode::Char('G') | KeyCode::End => self.set_cursor(last, false),
            KeyCode::Char('j') | KeyCode::Down if self.crosses(true) => {
                self.set_cursor(Part::Branches, true);
            }
            KeyCode::Char('k') | KeyCode::Up if self.crosses(false) => {
                self.set_cursor(Part::Changes, false);
            }
            _ => {
                let height = self.list_height(self.cursor);
                return self.current().handle_key(key, height);
            }
        }
        AppCommand::Continue
    }

    /// Whether a move down (or up) leaves the cursor's section for the
    /// one below (or above).
    fn crosses(&self, down: bool) -> bool {
        let (from, to) = if down {
            (Part::Changes, Part::Branches)
        } else {
            (Part::Branches, Part::Changes)
        };
        self.cursor == from
            && self.section_ref(from).is_none_or(|s| s.at_edge(down))
            && self.has_selectable(to)
    }

    /// The section whose rows lie under `position`.
    fn part_at(&self, position: Position) -> Option<Part> {
        let changes = Rect {
            x: self.changes_body.x.saturating_sub(GUTTER),
            width: self.changes_body.width + GUTTER,
            ..self.changes_body
        };
        if changes.contains(position) {
            Some(Part::Changes)
        } else if self.branches_body.contains(position) {
            Some(Part::Branches)
        } else {
            None
        }
    }

    /// `╭┄ [uncommitted]`, with `(no changes)` on a clean tree.
    fn header(&self, theme: &Theme) -> Line<'static> {
        let mut spans = vec![
            Span::raw("╭┄ ["),
            Span::styled(
                "uncommitted",
                Style::default().fg(rgb_to_color(theme.status_modified)),
            ),
            Span::raw("]"),
        ];
        if let Some(note) = self.changes.section_note() {
            let muted = Style::default().fg(rgb_to_color(theme.muted));
            spans.extend([Span::raw(" "), Span::styled(note, muted)]);
        }
        Line::from(spans)
    }

    /// Draw both sections into the pane's `body`.
    fn draw_sections(
        &mut self,
        frame: &mut Frame<'_>,
        body: Rect,
        scrollbar_x: u16,
        focused: bool,
        theme: &Theme,
    ) {
        let branch_rows = self.branches.as_ref().map_or(0, Section::list_rows);
        let file_rows = self.changes.list_rows();
        let changes_height = split(body.height as usize, file_rows + 2, branch_rows) as u16;
        let buffer = frame.buffer_mut();
        if changes_height > 0 {
            buffer.set_line(body.x, body.y, &self.header(theme), body.width);
        }
        let files = Rect {
            x: body.x + GUTTER.min(body.width),
            y: body.y + 1.min(changes_height),
            width: body.width.saturating_sub(GUTTER),
            height: changes_height.saturating_sub(2),
        };
        for y in files.y..files.bottom() + u16::from(changes_height > 1) {
            buffer.set_string(body.x, y, "┊", Style::default());
        }
        self.changes_body = files;
        let list = ListArea {
            body: files,
            scrollbar_x,
            cursor_here: self.cursor == Part::Changes,
            focused,
        };
        self.changes.draw_list(frame, list, theme);
        self.branches_body = Rect {
            y: body.y + changes_height,
            height: body.height - changes_height,
            ..body
        };
        let list = ListArea {
            body: self.branches_body,
            scrollbar_x,
            cursor_here: self.cursor == Part::Branches,
            focused,
        };
        match self.branches.as_mut() {
            Some(branches) => branches.draw_list(frame, list, theme),
            None => {
                let muted = Style::default().fg(rgb_to_color(theme.muted));
                let message = Paragraph::new("Not a git repository.").style(muted);
                frame.render_widget(message, self.branches_body);
            }
        }
    }
}

/// How many of `height` rows the changes section gets when it wants
/// `changes` rows and the branches want `branches`: what it wants when
/// both fit, else at least half, or all the branches leave over.
fn split(height: usize, changes: usize, branches: usize) -> usize {
    if changes + branches <= height {
        return changes.min(height);
    }
    changes.min((height / 2).max(height.saturating_sub(branches)))
}

impl Panel for StatusPanel {
    fn label(&self) -> String {
        match &self.branches {
            Some(branches) if branches.drilled_in() => branches.label(),
            _ => "Status".to_string(),
        }
    }

    fn draw_side(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        title: Line<'static>,
        focused: bool,
        theme: &Theme,
    ) {
        if self.drilled_in() {
            let branches = self.branches.as_mut().expect("drilled in");
            branches.draw_side(frame, area, title, focused, theme);
            return;
        }
        let footer = self
            .section_ref(self.cursor)
            .and_then(|section| section.footer(theme));
        let badges = self.changes.section_badges(theme);
        let chrome = pane::Chrome::new(title, focused)
            .badges(badges)
            .footer(footer);
        let body = pane::draw(frame, area, chrome, theme);
        let scrollbar_x = area.right().saturating_sub(1);
        self.draw_sections(frame, body, scrollbar_x, focused, theme);
    }

    fn draw_main(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        self.main_rect = area;
        self.current()
            .draw_main(frame, area, focused, theme, budget);
    }

    fn focus_main(&mut self, main: bool) {
        self.current().focus_main(main);
    }

    fn main_focused(&self) -> bool {
        self.current_ref().main_focused()
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        if self.drilled_in() || self.main_focused() || self.captures_text_input() {
            return self.current().handle_key(key, height);
        }
        self.handle_side_key(key)
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        if !key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            return self.handle_key(key.code, height);
        }
        self.current().handle_key_event(key, height)
    }

    fn escape(&mut self) -> bool {
        self.current().escape()
    }

    /// An event over a section's rows moves the cursor there first.
    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        let position = Position::new(mouse.column, mouse.row);
        if !self.drilled_in()
            && !self.main_rect.contains(position)
            && mouse.kind != MouseEventKind::Moved
        {
            let Some(part) = self.part_at(position) else {
                return AppCommand::Continue;
            };
            if part != self.cursor && self.has_selectable(part) {
                self.jump(part);
            }
            let height = self.list_height(part);
            return self.current().handle_mouse(mouse, height);
        }
        self.current().handle_mouse(mouse, height)
    }

    fn background(&mut self, active: bool) -> BackgroundWork {
        let mut work = self.changes.background(active);
        if let Some(branches) = self.branches.as_mut() {
            let more = branches.background(active);
            work.changed |= more.changed;
            work.pending |= more.pending;
        }
        work
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.current_ref().reserves_key(key)
    }

    fn captures_text_input(&self) -> bool {
        self.current_ref().captures_text_input()
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        self.current().report_copy(result);
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.current_ref().selected_path()
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn both_sections_get_what_they_want_when_it_fits() {
        assert_eq!(split(20, 5, 10), 5);
        assert_eq!(split(20, 2, 3), 2);
    }

    #[test]
    fn a_crowded_pane_gives_changes_half_or_what_branches_leave() {
        assert_eq!(split(20, 30, 4), 16);
        assert_eq!(split(20, 30, 30), 10);
        assert_eq!(split(20, 6, 30), 6);
    }
}
