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
//! Layout: a feed list (left column) plus the selected entry's diff
//! (right pane), modeled on Traces mode.
//!
//! ## Module layout
//!
//! Split by change axis. This file is the mode adapter: state, keys,
//! mouse, render, watch, reload. Each pane owns its slice:
//!
//! - [`model`]: the engine ([`model::LiveFeed`]) and its `ingest`.
//! - [`feed_pane`]: the feed list slice.
//! - [`detail`]: the detail/diff slice (cache + render).

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::ListState;

use deltoids::{Theme, git};

use crate::cli::browse::watch::{path_warrants_reload, spawn_workdir_watcher};
use crate::scroll::{ScrollDir, ScrollKind, WheelScroll};

use super::mode::{AppCommand, Mode, ReloadViewport, TabStrip};

mod detail;
mod feed_pane;
mod model;
#[cfg(test)]
mod test_support;

use detail::{DiffCache, max_detail_scroll, render_diff_pane};
use feed_pane::render_feed_pane;
use model::LiveFeed;

const DIFF_SCROLL_STEP: usize = 3;
const DIFF_MOUSE_SCROLL_STEP: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Feed,
    Diff,
}

/// Live-mode state plus the feed it renders.
pub(super) struct LiveMode {
    feed: LiveFeed,
    /// Selected feed index.
    selected: usize,
    /// Whether the selection follows the newest entry as the feed grows.
    /// True while the user is pinned to the tail; false once they scroll
    /// up. Keeps appends from yanking the view.
    follow_tail: bool,
    detail_scroll: usize,
    diff_cache: Option<DiffCache>,
    focus: Focus,
    feed_list_state: ListState,
    /// Last-drawn pane rects, for mouse hit-testing.
    feed_rect: Rect,
    diff_rect: Rect,
    wheel: WheelScroll<Focus>,
    /// Keeps the working-tree watcher alive for the session.
    _watcher: Option<notify::RecommendedWatcher>,
}

impl LiveMode {
    /// A cheap empty Live mode: no repo, empty feed. Used as the startup
    /// placeholder and the degraded fallback.
    pub(super) fn empty() -> Self {
        Self::from_feed(LiveFeed::empty())
    }

    /// Build the Live mode from the discovered repo, ingesting the
    /// current working-tree state once so a dirty repo shows initial
    /// entries. Degrades to an empty feed outside a repo or on git error.
    pub(super) fn build() -> Result<Self, String> {
        let mut feed = LiveFeed::new(git::Repo::discover());
        feed.ingest()?;
        let mut mode = Self::from_feed(feed);
        mode.selected = mode.feed.entries.len().saturating_sub(1);
        mode.feed_list_state.select(Some(mode.selected));
        Ok(mode)
    }

    fn from_feed(feed: LiveFeed) -> Self {
        Self {
            feed,
            selected: 0,
            follow_tail: true,
            detail_scroll: 0,
            diff_cache: None,
            focus: Focus::Feed,
            feed_list_state: ListState::default().with_selected(Some(0)),
            feed_rect: Rect::default(),
            diff_rect: Rect::default(),
            wheel: WheelScroll::new(),
            _watcher: None,
        }
    }

    fn detail_row_count(&self) -> usize {
        self.diff_cache.as_ref().map(|c| c.lines.len()).unwrap_or(0)
    }

    fn select(&mut self, index: usize) {
        let len = self.feed.entries.len();
        if len == 0 {
            return;
        }
        self.selected = index.min(len - 1);
        self.follow_tail = self.selected + 1 == len;
        self.feed_list_state.select(Some(self.selected));
        self.detail_scroll = 0;
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.feed.entries.len() {
            self.select(self.selected + 1);
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.select(self.selected - 1);
        }
    }
}

fn handle_key(state: &mut LiveMode, key: KeyCode, detail_height: usize) -> AppCommand {
    let rows = state.detail_row_count();
    match key {
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Feed => Focus::Diff,
                Focus::Diff => Focus::Feed,
            };
        }
        KeyCode::Char('1') => state.focus = Focus::Feed,
        KeyCode::Char('2') => state.focus = Focus::Diff,
        KeyCode::Char('J') => {
            let max = max_detail_scroll(rows, detail_height);
            state.detail_scroll = (state.detail_scroll + DIFF_SCROLL_STEP).min(max);
        }
        KeyCode::Char('K') => {
            state.detail_scroll = state.detail_scroll.saturating_sub(DIFF_SCROLL_STEP);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if state.focus == Focus::Diff {
                let max = max_detail_scroll(rows, detail_height);
                state.detail_scroll = (state.detail_scroll + DIFF_SCROLL_STEP).min(max);
            } else {
                state.move_down();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.focus == Focus::Diff {
                state.detail_scroll = state.detail_scroll.saturating_sub(DIFF_SCROLL_STEP);
            } else {
                state.move_up();
            }
        }
        KeyCode::PageDown => {
            let max = max_detail_scroll(rows, detail_height);
            state.detail_scroll = (state.detail_scroll + detail_height.max(1)).min(max);
        }
        KeyCode::PageUp => {
            state.detail_scroll = state.detail_scroll.saturating_sub(detail_height.max(1));
        }
        _ => {}
    }
    AppCommand::Continue
}

fn pane_at(state: &LiveMode, col: u16, row: u16) -> Option<Focus> {
    let pos = Position::new(col, row);
    if state.feed_rect.contains(pos) {
        Some(Focus::Feed)
    } else if state.diff_rect.contains(pos) {
        Some(Focus::Diff)
    } else {
        None
    }
}

fn handle_mouse(state: &mut LiveMode, mouse: MouseEvent, detail_height: usize) -> AppCommand {
    let Some(target) = pane_at(state, mouse.column, mouse.row) else {
        return AppCommand::Continue;
    };
    let rows = state.detail_row_count();

    match mouse.kind {
        MouseEventKind::ScrollDown => match target {
            Focus::Feed => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::List);
                for _ in 0..steps {
                    state.move_down();
                }
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Down, ScrollKind::Content);
                let max = max_detail_scroll(rows, detail_height);
                state.detail_scroll =
                    (state.detail_scroll + steps * DIFF_MOUSE_SCROLL_STEP).min(max);
            }
        },
        MouseEventKind::ScrollUp => match target {
            Focus::Feed => {
                let steps = state.wheel.advance(target, ScrollDir::Up, ScrollKind::List);
                for _ in 0..steps {
                    state.move_up();
                }
            }
            Focus::Diff => {
                let steps = state
                    .wheel
                    .advance(target, ScrollDir::Up, ScrollKind::Content);
                state.detail_scroll = state
                    .detail_scroll
                    .saturating_sub(steps * DIFF_MOUSE_SCROLL_STEP);
            }
        },
        MouseEventKind::Down(MouseButton::Left) => {
            state.focus = target;
            if target == Focus::Feed {
                let rect = state.feed_rect;
                let content_y = mouse.row.saturating_sub(rect.y).saturating_sub(1) as usize;
                let clicked = state.feed_list_state.offset() + content_y;
                if clicked < state.feed.entries.len() {
                    state.select(clicked);
                }
            }
        }
        _ => {}
    }
    AppCommand::Continue
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
        self.feed_rect = left;
        self.diff_rect = right;

        let feed_focused = self.focus == Focus::Feed;
        let border = deltoids::render_tui::pane_border_color(feed_focused, theme);
        let title: Line<'static> = tabs.title_line(border, theme);
        render_feed_pane(
            frame,
            left,
            &self.feed.entries,
            self.selected,
            &mut self.feed_list_state,
            feed_focused,
            title,
            theme,
        );
        let entry = self.feed.entries.get(self.selected);
        render_diff_pane(
            frame,
            right,
            entry,
            self.selected,
            &mut self.diff_cache,
            self.detail_scroll,
            self.focus,
            theme,
        );
    }

    fn handle_key(
        &mut self,
        key: KeyCode,
        _left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        handle_key(self, key, right_viewport)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _left_viewport: usize,
        right_viewport: usize,
    ) -> AppCommand {
        handle_mouse(self, mouse, right_viewport)
    }

    fn watch(&mut self) -> Option<Receiver<Vec<PathBuf>>> {
        let repo = self.feed.repo()?;
        let (watcher, rx) = spawn_workdir_watcher(repo).ok()?;
        self._watcher = watcher;
        Some(rx)
    }

    fn should_reload(&self, paths: &[PathBuf]) -> bool {
        match self.feed.repo() {
            Some(repo) => path_warrants_reload(repo, paths),
            None => false,
        }
    }

    fn needs_git_poll(&self) -> bool {
        false
    }

    fn reload(&mut self, _viewport: ReloadViewport, _theme: &Theme) -> Result<bool, String> {
        let old_selected = self.selected;
        let changed = self.feed.ingest()?;
        let len = self.feed.entries.len();
        self.selected = if len == 0 {
            0
        } else if self.follow_tail {
            len - 1
        } else {
            self.selected.min(len - 1)
        };
        self.feed_list_state.select(Some(self.selected));
        if self.selected != old_selected {
            self.detail_scroll = 0;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::live::test_support::*;
    use deltoids::Diff;
    use model::FeedEntry;

    fn mode_with_entries(n: usize) -> LiveMode {
        let mut mode = LiveMode::empty();
        for i in 0..n {
            mode.feed.entries.push(FeedEntry {
                path: format!("f{i}.txt"),
                timestamp: "00:00:00".to_string(),
                diff: Diff::compute("a\n", "a\nb\n", "f.txt"),
            });
        }
        mode.selected = n.saturating_sub(1);
        mode.follow_tail = true;
        mode.feed_list_state.select(Some(mode.selected));
        mode
    }

    #[test]
    fn tab_toggles_focus() {
        let mut mode = mode_with_entries(1);
        assert_eq!(mode.focus, Focus::Feed);
        handle_key(&mut mode, KeyCode::Tab, 10);
        assert_eq!(mode.focus, Focus::Diff);
        handle_key(&mut mode, KeyCode::Tab, 10);
        assert_eq!(mode.focus, Focus::Feed);
    }

    #[test]
    fn j_k_move_feed_selection() {
        let mut mode = mode_with_entries(3);
        mode.select(0);
        assert_eq!(mode.selected, 0);
        handle_key(&mut mode, KeyCode::Char('j'), 10);
        assert_eq!(mode.selected, 1);
        handle_key(&mut mode, KeyCode::Char('k'), 10);
        assert_eq!(mode.selected, 0);
    }

    #[test]
    fn moving_up_unpins_from_tail() {
        let mut mode = mode_with_entries(3);
        assert!(mode.follow_tail, "starts pinned to newest");
        handle_key(&mut mode, KeyCode::Char('k'), 10);
        assert!(!mode.follow_tail, "scrolling up unpins the tail");
        // Selecting the last entry re-pins.
        handle_key(&mut mode, KeyCode::Char('j'), 10);
        assert!(mode.follow_tail);
    }

    #[test]
    fn watch_and_should_reload_are_inert_without_repo() {
        let mut mode = LiveMode::empty();
        assert!(mode.watch().is_none());
        assert!(!mode.should_reload(&[PathBuf::from("src/main.rs")]));
        assert!(!mode.needs_git_poll());
    }

    #[test]
    fn reload_appends_and_follows_tail_when_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let mut feed = LiveFeed::new(git::Repo::discover_at(dir.path()));
        feed.ingest().unwrap(); // clean: empty
        let mut mode = LiveMode::from_feed(feed);

        // Edit and reload: one entry appended, selection follows tail.
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        stage_all(&repo);
        let changed = mode
            .reload(ReloadViewport::default(), &Theme::default())
            .unwrap();
        assert!(changed);
        assert_eq!(mode.feed.entries.len(), 1);
        assert_eq!(mode.selected, 0);
        assert!(mode.follow_tail);
    }

    #[test]
    fn draw_renders_feed_and_diff() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::{Constraint, Direction, Layout};

        let mut mode = LiveMode::empty();
        mode.feed.entries.push(FeedEntry {
            path: "src/app.rs".to_string(),
            timestamp: "08:15:00".to_string(),
            diff: Diff::compute("one\n", "one\ntwo\n", "src/app.rs"),
        });
        mode.select(0);

        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| {
            let area = f.area();
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(30), Constraint::Min(10)])
                .split(area);
            mode.draw(f, cols[0], cols[1], TabStrip { active: 2 }, &theme);
        })
        .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Live"), "tab strip missing Live: {text}");
        assert!(text.contains("app.rs"), "feed row missing file: {text}");
        assert!(text.contains("two"), "diff pane missing added line: {text}");
    }

    #[test]
    fn reload_preserves_selection_when_not_at_tail() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "one\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let mut feed = LiveFeed::new(git::Repo::discover_at(dir.path()));
        // Two initial dirty entries.
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "one\ntwo\n").unwrap();
        stage_all(&repo);
        feed.ingest().unwrap();
        let mut mode = LiveMode::from_feed(feed);
        assert_eq!(mode.feed.entries.len(), 2);

        // Scroll up to entry 0 (unpins tail).
        mode.select(0);
        assert!(!mode.follow_tail);

        // A new change appends a third entry; selection must stay on 0.
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        stage_all(&repo);
        mode.reload(ReloadViewport::default(), &Theme::default())
            .unwrap();
        assert_eq!(mode.feed.entries.len(), 3);
        assert_eq!(mode.selected, 0, "selection must not jump on append");
    }
}
