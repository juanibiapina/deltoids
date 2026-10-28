//! Read-only snapshots of history: one commit, or every commit of a
//! branch as one diff.
//!
//! [`SnapshotPanel`] reads the patch, resolves both sides of every file,
//! and diffs them on a worker thread, saying `Loading…` until the build
//! is ready. Then every [`Panel`] call goes to the snapshot's read-only
//! [`FilesMode`]: file tree and diff, with comments and hunk reshaping
//! but no staging, discarding, or reload. The file tree starts on a row
//! for the commit or branch, whose diff is its commits' `git show`
//! headers alone. A preview never asks Jev for judgments; a review does.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use deltoids::render_tui::{pane_block_with_title_line, pane_border_color, rgb_to_color};
use deltoids::{Theme, git};

use super::model::{self, Model};
use super::reload::Patches;
use super::{FilesMode, review};
use crate::cli::browse::diff_scrollbar;
use crate::cli::browse::mode::{AppCommand, BackgroundWork, DrawBudget};
use crate::cli::browse::panel::{Panel, panel_title};
use crate::cli::browse::vcs::Branch;
use crate::sidebar::Root;

/// What a snapshot shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::cli::browse) enum Snapshot {
    Commit(git::Commit),
    /// A branch's own commits as one diff, from `base` (the oldest
    /// commit's parent) to `tip` (the newest commit).
    Branch {
        name: String,
        base: String,
        tip: String,
        /// The commits between, newest first.
        commits: Vec<git::Commit>,
    },
}

impl Snapshot {
    /// The review of a branch whose own commits are known: an applied
    /// GitButler branch. `None` for a branch without them.
    pub(in crate::cli::browse) fn of_branch(branch: &Branch) -> Option<Self> {
        let tip = branch.commits.first()?;
        let oldest = branch.commits.last()?;
        Some(Self::Branch {
            name: branch.name.clone(),
            base: format!("{}^", oldest.commit.id),
            tip: tip.commit.id.clone(),
            commits: branch.commits.iter().map(|c| c.commit.clone()).collect(),
        })
    }

    /// Every commit the snapshot shows, newest first.
    pub(in crate::cli::browse) fn commits(&self) -> &[git::Commit] {
        match self {
            Self::Commit(commit) => std::slice::from_ref(commit),
            Self::Branch { commits, .. } => commits,
        }
    }

    /// `Commit <short id>` or `Review <branch>`.
    pub(in crate::cli::browse) fn label(&self) -> String {
        match self {
            Self::Commit(commit) => format!("Commit {}", commit.short_id),
            Self::Branch { name, .. } => format!("Review {name}"),
        }
    }

    /// The file tree's root row: the commit's subject, or the branch and
    /// how many commits it has.
    fn root(&self) -> Root {
        match self {
            Self::Commit(commit) => Root::Commit {
                subject: commit.summary.clone(),
            },
            Self::Branch { name, commits, .. } => Root::Branch {
                name: name.clone(),
                commits: commits.len(),
            },
        }
    }

    fn patch(&self, repo: &git::Repo) -> Result<String, String> {
        match self {
            Self::Commit(commit) => repo.commit_patch(&commit.id),
            Self::Branch { base, tip, .. } => repo.tree_patch(base, tip),
        }
    }
}

/// A finished build: the snapshot's model and the checkout its paths
/// live in (`None` for a bare repository).
type Build = Result<(Model, Option<PathBuf>), String>;

/// A snapshot shown as a panel, built on a worker thread.
pub(in crate::cli::browse) struct SnapshotPanel {
    snapshot: Snapshot,
    /// The build until the view takes it.
    receiver: Option<mpsc::Receiver<Build>>,
    built: Option<Build>,
    view: Option<Result<FilesMode, String>>,
    /// The view was adopted since the last [`Panel::background`]: a draw
    /// may have painted part of the frame as `Loading…`.
    adopted: bool,
    /// Handed to the view once built; `None` for a preview.
    review: Option<review::Review>,
    main_focused: bool,
    whole: bool,
    /// Width of the diff body the view builds at.
    diff_width: usize,
    /// Language servers the view navigates with.
    servers: Option<(crate::lsp::Lsp, PathBuf)>,
    /// The checkout the snapshot's paths live in, once built: custom
    /// commands run on the selected file there.
    workdir: Option<PathBuf>,
}

impl SnapshotPanel {
    /// A preview of `snapshot` from the repository at `repo_dir`.
    pub(in crate::cli::browse) fn preview(repo_dir: PathBuf, snapshot: Snapshot) -> Self {
        Self::new(repo_dir, snapshot, None)
    }

    /// A review of `snapshot`: a preview that asks Jev about its hunks
    /// when `TYPESAFE_API_KEY` is set.
    pub(in crate::cli::browse) fn review(repo_dir: PathBuf, snapshot: Snapshot) -> Self {
        Self::new(repo_dir, snapshot, Some(review::Review::from_env()))
    }

    pub(super) fn new(
        repo_dir: PathBuf,
        snapshot: Snapshot,
        review: Option<review::Review>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let job = snapshot.clone();
        std::thread::spawn(move || {
            let built = git::Repo::discover_at(&repo_dir)
                .ok_or_else(|| "Not a git repository".to_string())
                .and_then(|repo| {
                    let model = model::build_snapshot_model(&job.patch(&repo)?, &repo)?;
                    Ok((model, repo.workdir().map(Path::to_path_buf)))
                });
            let _ = sender.send(built);
        });
        Self {
            snapshot,
            receiver: Some(receiver),
            built: None,
            view: None,
            adopted: false,
            review,
            main_focused: false,
            whole: false,
            diff_width: 0,
            servers: None,
            workdir: None,
        }
    }

    /// Navigate the snapshot with `servers`, shared with the Files panel.
    pub(in crate::cli::browse) fn with_servers(
        mut self,
        servers: Option<(crate::lsp::Lsp, PathBuf)>,
    ) -> Self {
        self.servers = servers;
        self
    }

    /// Whether this panel shows `snapshot`: the same commit, or the same
    /// branch at the same commits.
    pub(in crate::cli::browse) fn shows(&self, snapshot: &Snapshot) -> bool {
        &self.snapshot == snapshot
    }

    /// Show the whole snapshot, as `git show` does: its commits' header
    /// above every file's diff. Off, the diff follows the file tree.
    pub(in crate::cli::browse) fn show_whole(&mut self, whole: bool) {
        self.whole = whole;
        if let Some(view) = self.view() {
            view.show_whole(whole);
        }
    }

    /// Whether the build finished and waits to be adopted. Never blocks.
    fn poll(&mut self) -> bool {
        if let Some(receiver) = &self.receiver {
            self.built = match receiver.try_recv() {
                Ok(built) => Some(built),
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Building the diff failed".to_string()))
                }
            };
            self.receiver = None;
        }
        self.built.is_some()
    }

    /// Take the finished build as the view. Never waits for it.
    pub(super) fn adopt(&mut self, theme: &Theme) {
        if !self.poll() {
            return;
        }
        let built = self.built.take().expect("polled above");
        self.view = Some(built.map(|(model, workdir)| {
            self.workdir = workdir;
            self.build_view(model, theme)
        }));
        self.adopted = true;
    }

    fn build_view(&mut self, model: Model, theme: &Theme) -> FilesMode {
        let mut view = FilesMode::new(model, Patches::default(), None, theme, self.diff_width);
        view.label = self.snapshot.label();
        view.read_only = true;
        view.attach_servers(self.servers.clone());
        view.show_snapshot(self.snapshot.root(), self.snapshot.commits().to_vec());
        if let Some(review) = self.review.take() {
            view.review = review;
            view.start_review();
        }
        view.focus_main(self.main_focused);
        view.show_whole(self.whole);
        view
    }

    pub(super) fn view(&mut self) -> Option<&mut FilesMode> {
        self.view.as_mut()?.as_mut().ok()
    }

    fn view_ref(&self) -> Option<&FilesMode> {
        self.view.as_ref()?.as_ref().ok()
    }

    /// `Loading…`, or the build's error, in a pane under `title`.
    fn draw_message(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        title: Line<'static>,
        focused: bool,
        theme: &Theme,
    ) {
        let text = match &self.view {
            Some(Err(err)) => err.clone(),
            _ => "Loading…".to_string(),
        };
        let color = pane_border_color(focused, theme);
        let block = pane_block_with_title_line(title, color, None);
        let muted = Style::default().fg(rgb_to_color(theme.muted));
        frame.render_widget(Paragraph::new(text).style(muted).block(block), area);
    }
}

impl Panel for SnapshotPanel {
    fn label(&self) -> String {
        self.snapshot.label()
    }

    fn draw_side(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        title: Line<'static>,
        focused: bool,
        theme: &Theme,
    ) {
        self.adopt(theme);
        if let Some(view) = self.view() {
            view.draw_side(frame, area, title, focused, theme);
            return;
        }
        self.draw_message(frame, area, title, focused, theme);
    }

    fn draw_main(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &Theme,
        budget: DrawBudget,
    ) {
        self.diff_width = diff_scrollbar::body_area(area).width as usize;
        self.adopt(theme);
        match self.view() {
            Some(view) => view.draw_main(frame, area, focused, theme, budget),
            None => {
                let title = panel_title(pane_border_color(focused, theme), "Diff", theme);
                self.draw_message(frame, area, title, focused, theme);
            }
        }
    }

    fn focus_main(&mut self, main: bool) {
        self.main_focused = main;
        if let Some(view) = self.view() {
            view.focus_main(main);
        }
    }

    fn main_focused(&self) -> bool {
        self.view_ref()
            .map_or(self.main_focused, Panel::main_focused)
    }

    fn handle_key(&mut self, key: KeyCode, height: usize) -> AppCommand {
        let Some(view) = self.view() else {
            return AppCommand::Continue;
        };
        let command = Panel::handle_key(view, key, height);
        self.main_focused = view.main_focused();
        command
    }

    fn handle_key_event(&mut self, key: KeyEvent, height: usize) -> AppCommand {
        let Some(view) = self.view() else {
            return AppCommand::Continue;
        };
        let command = Panel::handle_key_event(view, key, height);
        self.main_focused = view.main_focused();
        command
    }

    fn escape(&mut self) -> bool {
        self.view().is_some_and(Panel::escape)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, height: usize) -> AppCommand {
        let Some(view) = self.view() else {
            return AppCommand::Continue;
        };
        let command = Panel::handle_mouse(view, mouse, height);
        self.main_focused = view.main_focused();
        command
    }

    /// While building: pending until the build is ready to adopt, then
    /// changed once so the next frame adopts it. A panel off screen keeps
    /// its build until a draw adopts it. Changed once more after a draw
    /// adopts it, since the build can land between the side and the main
    /// half of one frame.
    fn background(&mut self, active: bool) -> BackgroundWork {
        if self.view.is_none() {
            let announced = self.built.is_some();
            let ready = self.poll();
            return BackgroundWork {
                changed: ready && !announced,
                pending: !ready,
            };
        }
        let adopted = std::mem::take(&mut self.adopted);
        let work = self.view().map_or_else(BackgroundWork::default, |view| {
            Panel::background(view, active)
        });
        BackgroundWork {
            changed: work.changed || adopted,
            ..work
        }
    }

    fn reserves_key(&self, key: KeyCode) -> bool {
        self.view_ref()
            .is_some_and(|view| Panel::reserves_key(view, key))
    }

    fn captures_text_input(&self) -> bool {
        self.view_ref().is_some_and(Panel::captures_text_input)
    }

    fn report_copy(&mut self, result: Result<(), String>) {
        if let Some(view) = self.view() {
            Panel::report_copy(view, result);
        }
    }

    fn selected_path(&self) -> Option<PathBuf> {
        let file = self.view_ref()?.selected_file()?;
        Some(self.workdir.as_ref()?.join(file))
    }
}
