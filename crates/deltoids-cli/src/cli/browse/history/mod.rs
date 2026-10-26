//! The branches panel, in the spirit of lazygit's.
//!
//! [`History`] shows what [`Vcs::overview`] gives: the local branches,
//! the current one first, or a GitButler workspace's branches drawn like
//! `but status` (stacks of branches with their commits, closed by the
//! common base) with the unapplied branches below
//! ([`tree`]). Uncommitted files
//! stay in the Files panel. The cursor stops on branches and commits. On
//! a branch the main view shows its log; on a commit, a preview that
//! reads like `git show` (header, message, every file's diff), built as a
//! read-only [`SnapshotPanel`]. `Enter` on a commit asks the workspace
//! to review it in the Files panel, and `Enter` on an applied GitButler
//! branch asks for a review of all its commits as one diff
//! ([`History::take_review_request`]). `Enter` on any other branch lists
//! the branch's commits in the same panel, newest first and loaded a page
//! at a time, and the main view previews the selected commit's diff.
//! `Esc` steps back up to the branches, keeping the selection. A double
//! click on a branch or commit does what `Enter` does on it.
//!
//! The lists are a snapshot; [`History::refresh`] re-reads them and keeps
//! each selection on the same branch or commit.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use deltoids::render_tui::{
    pane_block_with_title_line, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};
use deltoids::{Theme, git};

use super::double_click::DoubleClick;
use super::files::{Snapshot, SnapshotPanel};
use super::mode::{AppCommand, BackgroundWork, DrawBudget};
use super::panel::{Panel, panel_title};
use super::vcs::{Branch, Vcs};

mod cursor;
mod tree;

use cursor::Cursor;
use tree::Tree;

/// How many commits one page of a commit list loads.
const PAGE: usize = 200;

/// Which step of the drill-down the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Branches,
    Commits,
}

/// The commits of one branch, loaded a page at a time.
struct CommitList {
    branch: Branch,
    label: String,
    commits: Result<Vec<git::Commit>, String>,
    /// Whether the last load returned fewer commits than it asked for.
    exhausted: bool,
    cursor: Cursor,
}

impl CommitList {
    fn load(vcs: &Vcs, branch: Branch, label: String) -> Self {
        let commits = vcs.commits(&branch, PAGE);
        Self {
            exhausted: commits.as_ref().map_or(true, |c| c.len() < PAGE),
            branch,
            label,
            commits,
            cursor: Cursor::default(),
        }
    }

    fn len(&self) -> usize {
        self.commits.as_ref().map_or(0, Vec::len)
    }

    fn selected(&self) -> Option<&git::Commit> {
        self.commits.as_ref().ok()?.get(self.cursor.selected)
    }

    /// Load the next page once the selection reaches the last commit.
    fn load_more(&mut self, vcs: &Vcs) {
        if self.exhausted || self.cursor.selected + 1 < self.len() {
            return;
        }
        let want = self.len() + PAGE;
        if let Ok(more) = vcs.commits(&self.branch, want) {
            self.exhausted = more.len() < want;
            self.commits = Ok(more);
        }
    }

    /// Re-read as many commits as are loaded, keeping the selection on
    /// the same commit when it is still there.
    fn refresh(&mut self, vcs: &Vcs) {
        let selected = self.selected().map(|c| c.id.clone());
        let want = self.len().max(PAGE);
        self.commits = vcs.commits(&self.branch, want);
        self.exhausted = self.len() < want;
        if let Some(index) = selected.and_then(|id| {
            let commits = self.commits.as_ref().ok()?;
            commits.iter().position(|c| c.id == id)
        }) {
            self.cursor.selected = index;
        }
    }
}

/// A list pane's frame: its title, footer, whether it has focus, and
/// whether the cursor's row is highlighted.
struct Chrome {
    title: Line<'static>,
    footer: Option<Line<'static>>,
    focused: bool,
    highlight: bool,
}

pub(super) struct History {
    vcs: Vcs,
    screen: Screen,
    main_focused: bool,
    /// The branches screen's title.
    label: &'static str,
    tree: Result<Tree, String>,
    branch_cursor: Cursor,
    /// Each branch's log for the branches screen's main view.
    logs: HashMap<String, Result<Vec<git::Commit>, String>>,
    /// First log line on screen.
    log_scroll: usize,
    commits: Option<CommitList>,
    /// The selected commit's diff, previewed in the main view.
    preview: Option<SnapshotPanel>,
    /// A review `Enter` asked for, until the workspace takes it.
    review_request: Option<Snapshot>,
    double_click: DoubleClick,
    /// Seconds since the epoch at the last read; commit ages count from
    /// here.
    now: i64,
    side_rect: Rect,
    main_rect: Rect,
    /// Language servers commit previews navigate with.
    servers: Option<(crate::lsp::Lsp, std::path::PathBuf)>,
}

impl History {
    /// Navigate commit previews with `servers`, shared with the Files
    /// panel.
    pub(super) fn with_servers(
        mut self,
        servers: Option<(crate::lsp::Lsp, std::path::PathBuf)>,
    ) -> Self {
        self.servers = servers;
        self
    }

    /// The branches panel.
    pub(super) fn branches(mut vcs: Vcs) -> Self {
        let overview = vcs.overview();
        let tree = overview.content.map(Tree::new);
        let mut branch_cursor = Cursor::default();
        if let Ok(tree) = &tree {
            branch_cursor.selected = tree.snap(0, true);
        }
        Self {
            label: overview.label,
            tree,
            vcs,
            screen: Screen::Branches,
            main_focused: false,
            branch_cursor,
            logs: HashMap::new(),
            log_scroll: 0,
            commits: None,
            preview: None,
            review_request: None,
            double_click: DoubleClick::default(),
            now: now(),
            side_rect: Rect::default(),
            main_rect: Rect::default(),
            servers: None,
        }
    }

    /// Re-read branches and commits, keeping each selection on the same
    /// branch or commit when it still exists. An open commit stays open:
    /// a commit never changes.
    pub(super) fn refresh(&mut self) {
        self.now = now();
        let row = self.branch_cursor.selected;
        let key = self.tree.as_ref().ok().and_then(|tree| tree.key(row));
        let overview = self.vcs.overview();
        self.label = overview.label;
        self.tree = overview.content.map(Tree::new);
        if let Ok(tree) = &self.tree {
            self.branch_cursor.selected = key
                .and_then(|key| tree.find(&key))
                .unwrap_or_else(|| tree.snap(row, true));
        }
        self.logs.clear();
        if let Some(list) = self.commits.as_mut() {
            list.refresh(&self.vcs);
        }
    }

    fn branch_count(&self) -> usize {
        self.tree.as_ref().map_or(0, Tree::len)
    }

    fn selected_branch(&self) -> Option<&Branch> {
        self.tree.as_ref().ok()?.branch(self.branch_cursor.selected)
    }

    /// The commit selected on the branches screen.
    fn tree_commit(&self) -> Option<&git::Commit> {
        self.tree.as_ref().ok()?.commit(self.branch_cursor.selected)
    }

    /// The commit the main view shows: from the branches screen or from a
    /// branch's commit list.
    fn selected_commit(&self) -> Option<&git::Commit> {
        match self.screen {
            Screen::Branches => self.tree_commit(),
            Screen::Commits => self.commits.as_ref()?.selected(),
        }
    }

    /// The preview, while it shows the selected commit.
    fn preview(&mut self) -> Option<&mut SnapshotPanel> {
        let selected = Snapshot::Commit(self.selected_commit()?.clone());
        self.preview.as_mut().filter(|p| p.shows(&selected))
    }

    fn preview_ref(&self) -> Option<&SnapshotPanel> {
        let selected = Snapshot::Commit(self.selected_commit()?.clone());
        self.preview.as_ref().filter(|p| p.shows(&selected))
    }

    /// Start building the selected commit's preview, unless it is already
    /// there. Never waits for the build.
    fn ensure_preview(&mut self) {
        let Some(selected) = self.selected_commit().cloned() else {
            self.preview = None;
            return;
        };
        let selected = Snapshot::Commit(selected);
        if !self.preview.as_ref().is_some_and(|p| p.shows(&selected)) {
            let repo_dir = self.vcs.repo().git_dir().to_path_buf();
            let mut preview =
                SnapshotPanel::preview(repo_dir, selected).with_servers(self.servers.clone());
            // Keys reach the preview only from the main view, where it
            // reads like `git show`.
            preview.focus_main(true);
            preview.show_whole(true);
            self.preview = Some(preview);
        }
    }

    fn open_branch_commits(&mut self) {
        let Some(branch) = self.selected_branch().cloned() else {
            return;
        };
        if self
            .commits
            .as_ref()
            .is_none_or(|list| list.branch.name != branch.name)
        {
            let label = format!("Commits ({})", branch.name);
            self.commits = Some(CommitList::load(&self.vcs, branch, label));
        }
        self.screen = Screen::Commits;
    }

    /// `Enter` on a branch: an applied GitButler branch, whose commits the
    /// tree lists, asks for a review of all of them; any other lists its
    /// commits.
    fn open_branch(&mut self) {
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if branch.id.is_some() {
            self.review_request = Snapshot::of_branch(branch);
        } else {
            self.open_branch_commits();
        }
    }

    /// `Enter` on the selected row: a commit asks for its review, and a
    /// branch opens ([`Self::open_branch`]).
    fn open_selected(&mut self) {
        match self.selected_commit() {
            Some(commit) => self.review_request = Some(Snapshot::Commit(commit.clone())),
            None if self.screen == Screen::Branches => self.open_branch(),
            None => {}
        }
    }

    /// The review `Enter` asked for since the last call.
    pub(super) fn take_review_request(&mut self) -> Option<Snapshot> {
        self.review_request.take()
    }

    fn move_list(&mut self, key: KeyCode, height: usize) {
        match self.screen {
            Screen::Branches => {
                let len = self.branch_count();
                let before = self.branch_cursor.selected;
                self.branch_cursor.handle_key(key, len, height);
                if let Ok(tree) = &self.tree {
                    let row = self.branch_cursor.selected;
                    self.branch_cursor.selected = tree.snap(row, row >= before);
                }
                self.log_scroll = 0;
            }
            Screen::Commits => {
                if let Some(list) = self.commits.as_mut() {
                    let len = list.len();
                    list.cursor.handle_key(key, len, height);
                    list.load_more(&self.vcs);
                }
            }
        }
    }

    fn scroll_log(&mut self, key: KeyCode, height: usize) {
        let len = match self.branch_log() {
            Some(Ok(commits)) => commits.len(),
            _ => 0,
        };
        let last = len.saturating_sub(1);
        let page = height.max(1);
        self.log_scroll = match key {
            KeyCode::Char('j') | KeyCode::Down => self.log_scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.log_scroll.saturating_sub(1),
            KeyCode::PageDown => self.log_scroll.saturating_add(page),
            KeyCode::PageUp => self.log_scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => last,
            _ => return,
        }
        .min(last);
    }

    /// The selected branch's log, loaded on first view.
    fn branch_log(&mut self) -> Option<&Result<Vec<git::Commit>, String>> {
        let branch = self.selected_branch()?.clone();
        let vcs = &self.vcs;
        Some(
            self.logs
                .entry(branch.name.clone())
                .or_insert_with(|| vcs.commits(&branch, PAGE)),
        )
    }

    fn draw_branches(&mut self, frame: &mut Frame<'_>, area: Rect, chrome: Chrome, theme: &Theme) {
        let now = self.now;
        let (rows, highlight) = match &self.tree {
            Ok(tree) if tree.len() == 0 => (Err("No branches.".to_string()), false),
            Ok(tree) => (
                Ok(tree.lines(now, theme)),
                tree.is_selectable(self.branch_cursor.selected),
            ),
            Err(err) => (Err(err.clone()), false),
        };
        let chrome = Chrome {
            highlight,
            ..chrome
        };
        draw_list(frame, area, chrome, rows, &mut self.branch_cursor, theme);
    }

    fn draw_commits(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        title: Line<'static>,
        focused: bool,
        theme: &Theme,
    ) {
        let now = self.now;
        let footer = self.selected_commit().map(|c| commit_footer(c, now, theme));
        let Some(list) = self.commits.as_mut() else {
            return;
        };
        let rows = match &list.commits {
            Ok(commits) if commits.is_empty() => Err("No commits.".to_string()),
            Ok(commits) => Ok(commits.iter().map(|c| commit_row(c, now, theme)).collect()),
            Err(err) => Err(err.clone()),
        };
        let chrome = Chrome {
            title,
            footer,
            focused,
            highlight: true,
        };
        draw_list(frame, area, chrome, rows, &mut list.cursor, theme);
    }

    fn draw_branch_log(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool, theme: &Theme) {
        let now = self.now;
        let scroll = self.log_scroll;
        let lines = match self.branch_log() {
            Some(Ok(commits)) => commits
                .iter()
                .skip(scroll)
                .map(|c| commit_row(c, now, theme))
                .collect(),
            Some(Err(err)) => vec![Line::from(err.clone())],
            None => Vec::new(),
        };
        let color = pane_border_color(focused, theme);
        let block = pane_block_with_title_line(panel_title(color, "Log", theme), color, None);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_commit_diff(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        // Building a commit's diff costs real time: in the list, wait for
        // held keys to settle before starting the newly selected one.
        if budget == DrawBudget::Full {
            self.ensure_preview();
        }
        let selected = self.selected_commit().is_some();
        if let Some(preview) = self.preview() {
            preview.draw_main(frame, area, focused, theme, budget);
            return;
        }
        let color = pane_border_color(focused, theme);
        let text = if selected { "Loading…" } else { "" };
        let block = pane_block_with_title_line(panel_title(color, "Diff", theme), color, None);
        let muted = Style::default().fg(rgb_to_color(theme.muted));
        frame.render_widget(Paragraph::new(text).style(muted).block(block), area);
    }

    /// Select the list row under a click in the side panel. Returns the
    /// row it selected, or `None` when the click hit nothing selectable.
    fn click_row(&mut self, row: u16) -> Option<usize> {
        let inner = self.side_rect.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        if row < inner.y || row >= inner.bottom() {
            return None;
        }
        let offset = (row - inner.y) as usize;
        match self.screen {
            Screen::Branches => {
                let len = self.branch_count();
                let before = self.branch_cursor.selected;
                let hit = self.branch_cursor.click(offset, len);
                self.log_scroll = 0;
                let selectable = self
                    .tree
                    .as_ref()
                    .is_ok_and(|tree| tree.is_selectable(self.branch_cursor.selected));
                if !selectable {
                    self.branch_cursor.selected = before;
                }
                hit.filter(|_| selectable)
            }
            Screen::Commits => {
                let list = self.commits.as_mut()?;
                let len = list.len();
                let hit = list.cursor.click(offset, len);
                list.load_more(&self.vcs);
                hit
            }
        }
    }

    fn forward_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        self.preview().map_or(AppCommand::Continue, |preview| {
            preview.handle_key(key, height)
        })
    }
}

impl Panel for History {
    fn label(&self) -> String {
        match (self.screen, &self.commits) {
            (Screen::Commits, Some(list)) => list.label.clone(),
            _ => self.label.to_string(),
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
        self.side_rect = area;
        match self.screen {
            Screen::Branches => {
                let chrome = Chrome {
                    title,
                    footer: None,
                    focused,
                    highlight: true,
                };
                self.draw_branches(frame, area, chrome, theme);
            }
            Screen::Commits => self.draw_commits(frame, area, title, focused, theme),
        }
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
        match self.screen {
            Screen::Branches if self.tree_commit().is_none() => {
                self.draw_branch_log(frame, area, focused, theme);
            }
            Screen::Branches | Screen::Commits => {
                self.draw_commit_diff(frame, area, focused, theme, budget);
            }
        }
    }

    fn focus_main(&mut self, main: bool) {
        self.main_focused = main;
    }

    fn main_focused(&self) -> bool {
        self.main_focused
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        if self.captures_text_input() {
            return self.forward_key(key, height);
        }
        if self.main_focused {
            return match self.screen {
                Screen::Branches if self.tree_commit().is_none() => {
                    self.scroll_log(key, height);
                    AppCommand::Continue
                }
                Screen::Branches | Screen::Commits => self.forward_key(key, height),
            };
        }
        match key {
            KeyCode::Esc => self.screen = Screen::Branches,
            KeyCode::Enter => self.open_selected(),
            // Shift+J/K scroll the preview, as they scroll the diff from
            // the Files panel.
            KeyCode::Char('J' | 'K') => return self.forward_key(key, height),
            _ => self.move_list(key, height),
        }
        AppCommand::Continue
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        if self.main_focused && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.preview().map_or(AppCommand::Continue, |preview| {
                preview.handle_key_event(key, height)
            });
        }
        self.handle_key(key.code, height)
    }

    fn escape(&mut self) -> bool {
        self.main_focused && self.preview().is_some_and(Panel::escape)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        let double = self.double_click.press(&mouse);
        let position = Position::new(mouse.column, mouse.row);
        let in_main = self.main_rect.contains(position);
        let to_commit = match self.screen {
            Screen::Commits => in_main,
            Screen::Branches => in_main && self.tree_commit().is_some(),
        };
        if to_commit {
            return self.preview().map_or(AppCommand::Continue, |preview| {
                preview.handle_mouse(mouse, height)
            });
        }
        let scroll = |history: &mut Self, key| {
            if in_main {
                history.scroll_log(key, height);
            } else {
                history.move_list(key, height);
            }
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if !in_main => {
                if self.click_row(mouse.row).is_some() && double {
                    self.open_selected();
                }
            }
            MouseEventKind::ScrollDown => scroll(self, KeyCode::Down),
            MouseEventKind::ScrollUp => scroll(self, KeyCode::Up),
            _ => {}
        }
        AppCommand::Continue
    }

    fn background(&mut self, active: bool) -> BackgroundWork {
        self.preview()
            .map_or_else(BackgroundWork::default, |preview| {
                preview.background(active)
            })
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.preview_ref()
            .filter(|_| self.main_focused)
            .is_some_and(|preview| preview.reserves_key(key))
    }

    fn captures_text_input(&self) -> bool {
        self.preview_ref().is_some_and(Panel::captures_text_input)
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        if let Some(preview) = self.preview() {
            preview.report_copy(result);
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Draw a list pane: `rows`, or a muted message in their place, with the
/// cursor's row highlighted across the full width.
fn draw_list(
    frame: &mut Frame<'_>,
    area: Rect,
    chrome: Chrome,
    rows: Result<Vec<Line<'static>>, String>,
    cursor: &mut Cursor,
    theme: &Theme,
) {
    let color = pane_border_color(chrome.focused, theme);
    let block = pane_block_with_title_line(chrome.title, color, chrome.footer);
    let rows = match rows {
        Ok(rows) => rows,
        Err(message) => {
            let muted = Style::default().fg(rgb_to_color(theme.muted));
            frame.render_widget(Paragraph::new(message).style(muted).block(block), area);
            return;
        }
    };
    let height = pane_inner_height(area);
    let width = area.width.saturating_sub(2) as usize;
    let visible = cursor.visible(rows.len(), height);
    let selected_bg = Style::default().bg(rgb_to_color(theme.selection_bg));
    let lines: Vec<Line<'static>> = rows[visible.clone()]
        .iter()
        .zip(visible.clone())
        .map(|(row, index)| {
            if index != cursor.selected || !chrome.highlight {
                return row.clone();
            }
            let mut row = row.clone().patch_style(selected_bg);
            let pad = width.saturating_sub(row.width());
            row.spans.push(Span::styled(" ".repeat(pad), selected_bg));
            row
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
    render_pane_scrollbar(
        frame,
        area,
        rows.len(),
        cursor.selected,
        height,
        chrome.focused,
        theme,
    );
}

/// `  2d * main`: the tip's age, a star on the current branch.
fn branch_row(branch: &Branch, now: i64, theme: &Theme) -> Line<'static> {
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let (marker, name) = if branch.current {
        (
            "* ",
            Style::default()
                .fg(rgb_to_color(theme.status_added))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  ", Style::default())
    };
    Line::from(vec![
        Span::styled(format!("{:>3} ", age(now, branch.time)), muted),
        Span::styled(marker, name),
        Span::styled(branch.name.clone(), name),
    ])
}

/// `abc1234  2d subject`.
fn commit_row(commit: &git::Commit, now: i64, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} ", commit.short_id),
            Style::default().fg(rgb_to_color(theme.status_modified)),
        ),
        Span::styled(
            format!("{:>3} ", age(now, commit.time)),
            Style::default().fg(rgb_to_color(theme.muted)),
        ),
        Span::raw(commit.summary.clone()),
    ])
}

/// `Ada Lovelace · 2d ago`, under the commit list.
fn commit_footer(commit: &git::Commit, now: i64, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {} · {} ago ", commit.author, age(now, commit.time)),
        Style::default().fg(rgb_to_color(theme.muted)),
    ))
}

/// How long ago `time` was at `now`, in the largest whole unit: `45s`,
/// `3m`, `2h`, `4d`, `2w`, `5M`, `1y`. Times after `now` read as `0s`.
fn age(now: i64, time: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let seconds = (now - time).max(0);
    let days = seconds / DAY;
    match seconds {
        s if s < MINUTE => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        _ if days < 7 => format!("{days}d"),
        _ if days < 30 => format!("{}w", days / 7),
        _ if days < 365 => format!("{}M", days / 30),
        _ => format!("{}y", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::age;

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        assert_eq!(age(1_000, 1_000), "0s");
        assert_eq!(age(1_059, 1_000), "59s");
        assert_eq!(age(1_000 + 60, 1_000), "1m");
        assert_eq!(age(1_000 + 2 * 3600, 1_000), "2h");
        assert_eq!(age(1_000 + 3 * 86_400, 1_000), "3d");
        assert_eq!(age(1_000 + 14 * 86_400, 1_000), "2w");
        assert_eq!(age(1_000 + 60 * 86_400, 1_000), "2M");
        assert_eq!(age(1_000 + 800 * 86_400, 1_000), "2y");
        // A commit from the future (clock skew) reads as now.
        assert_eq!(age(1_000, 2_000), "0s");
    }
}
