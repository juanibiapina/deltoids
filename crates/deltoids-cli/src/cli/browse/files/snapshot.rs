//! Read-only snapshots of history: one commit, every commit of a branch
//! as one diff, or a GitHub pull request.
//!
//! [`SnapshotPanel`] resolves what it shows (a pull request asks GitHub
//! and fetches), reads the patch, resolves both sides of every file,
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

use deltoids::render_tui::rgb_to_color;
use deltoids::{Theme, git};

use super::model::{self, Model};
use super::reload::Patches;
use super::{FilesMode, ReviewMarks, review};
use crate::cli::browse::diff_scrollbar;
use crate::cli::browse::mode::{AppCommand, BackgroundWork, DrawBudget};
use crate::cli::browse::pane;
use crate::cli::browse::panel::{Panel, panel_title};
use crate::cli::browse::vcs::Branch;
use crate::pull_request::PullRequestInfo;
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
    /// A pull request's diff from `base` (the merge base) to `tip` (its
    /// head), as GitHub shows its changed files.
    PullRequest {
        /// `owner/repo#N`.
        name: String,
        info: PullRequestInfo,
        base: String,
        tip: String,
        /// Its commits, newest first.
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
            Self::Branch { commits, .. } | Self::PullRequest { commits, .. } => commits,
        }
    }

    /// `Commit <short id>` or `Review <branch>`.
    pub(in crate::cli::browse) fn label(&self) -> String {
        match self {
            Self::Commit(commit) => format!("Commit {}", commit.short_id),
            Self::Branch { name, .. } => format!("Review {name}"),
            Self::PullRequest { name, .. } => pull_request_label(name),
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
            Self::PullRequest { info, .. } => Root::PullRequest {
                number: info.number,
                title: info.title.clone(),
            },
        }
    }

    fn patch(&self, repo: &git::Repo) -> Result<String, String> {
        match self {
            Self::Commit(commit) => repo.commit_patch(&commit.id),
            Self::Branch { base, tip, .. } | Self::PullRequest { base, tip, .. } => {
                repo.tree_patch(base, tip)
            }
        }
    }
}

/// `PR owner/repo#N`.
pub(in crate::cli::browse) fn pull_request_label(name: &str) -> String {
    format!("PR {name}")
}

/// Work on the worker thread before the build: what to show, and the
/// repository to read it from. May block on the network. It hands a pull
/// request's description to the callback as soon as it has it, so the
/// panel shows it while the rest loads.
pub(in crate::cli::browse) type Resolve =
    Box<dyn FnOnce(&dyn Fn(&PullRequestInfo)) -> Result<(PathBuf, Snapshot), String> + Send>;

/// A finished build: the snapshot, its model, and the checkout its paths
/// live in (`None` for a bare repository).
type Build = Result<(Snapshot, Model, Option<PathBuf>), String>;

/// A snapshot shown as a panel, built on a worker thread.
pub(in crate::cli::browse) struct SnapshotPanel {
    /// The panel's title, also while the snapshot resolves.
    label: String,
    /// What the panel shows, once known.
    snapshot: Option<Snapshot>,
    /// The build until the view takes it.
    receiver: Option<mpsc::Receiver<Build>>,
    /// The pull request's description, until the build comes.
    early_receiver: Option<mpsc::Receiver<PullRequestInfo>>,
    /// What the main view shows while the build runs.
    early: Option<PullRequestInfo>,
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
    /// The session's reviewed marks.
    review_marks: ReviewMarks,
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
        let job = snapshot.clone();
        let resolve: Resolve = Box::new(move |_| Ok((repo_dir, job)));
        Self::spawn(snapshot.label(), Some(snapshot), resolve, review)
    }

    /// A review of what `resolve` finds, titled `label` until then.
    pub(in crate::cli::browse) fn resolving(label: String, resolve: Resolve) -> Self {
        Self::spawn(label, None, resolve, Some(review::Review::from_env()))
    }

    pub(super) fn spawn(
        label: String,
        snapshot: Option<Snapshot>,
        resolve: Resolve,
        review: Option<review::Review>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (early_sender, early_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let shown = |info: &PullRequestInfo| {
                let _ = early_sender.send(info.clone());
            };
            let built = resolve(&shown).and_then(|(repo_dir, snapshot)| {
                let repo = git::Repo::discover_at(&repo_dir)
                    .ok_or_else(|| "Not a git repository".to_string())?;
                let model = model::build_snapshot_model(&snapshot.patch(&repo)?, &repo)?;
                Ok((snapshot, model, repo.workdir().map(Path::to_path_buf)))
            });
            let _ = sender.send(built);
        });
        Self {
            label,
            snapshot,
            receiver: Some(receiver),
            early_receiver: Some(early_receiver),
            early: None,
            built: None,
            view: None,
            adopted: false,
            review,
            main_focused: false,
            whole: false,
            diff_width: 0,
            servers: None,
            review_marks: ReviewMarks::default(),
            workdir: None,
        }
    }

    /// Mark files reviewed with `marks`, shared with the Files panel.
    pub(in crate::cli::browse) fn with_review_marks(mut self, marks: ReviewMarks) -> Self {
        self.review_marks = marks;
        self
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
        self.snapshot.as_ref() == Some(snapshot)
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
        self.view = Some(built.map(|(snapshot, model, workdir)| {
            self.workdir = workdir;
            let view = self.build_view(&snapshot, model, theme);
            self.snapshot = Some(snapshot);
            view
        }));
        self.adopted = true;
    }

    fn build_view(&mut self, snapshot: &Snapshot, model: Model, theme: &Theme) -> FilesMode {
        let mut view = FilesMode::new(model, Patches::default(), None, theme, self.diff_width);
        view.label = snapshot.label();
        view.read_only = true;
        view.attach_servers(self.servers.clone());
        view.show_snapshot(snapshot.root(), snapshot.commits().to_vec());
        if let Snapshot::PullRequest { info, .. } = snapshot {
            view.show_pull_request(info.clone());
        }
        view.attach_review_marks(self.review_marks.clone());
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
            _ if self.early.is_some() => "Loading files…".to_string(),
            _ => "Loading…".to_string(),
        };
        let body = pane::draw(frame, area, pane::Chrome::new(title, focused), theme);
        let muted = Style::default().fg(rgb_to_color(theme.muted));
        frame.render_widget(Paragraph::new(text).style(muted), body);
    }

    /// The main view while the build runs: the pull request's header as
    /// the root row will show it, or `Loading…` before it comes.
    fn draw_early(&self, frame: &mut Frame<'_>, area: Rect, focused: bool, theme: &Theme) -> bool {
        let (None | Some(Ok(_)), Some(info)) = (&self.view, &self.early) else {
            return false;
        };
        let title = panel_title("Diff");
        let body = pane::draw(frame, area, pane::Chrome::new(title, focused), theme);
        let width = diff_scrollbar::body_area(area).width as usize;
        let lines = super::pr_header::lines(info, width, theme);
        frame.render_widget(Paragraph::new(lines), body);
        true
    }

    /// Take the pull request's description if it came. Never blocks.
    fn poll_early(&mut self) -> bool {
        let Some(receiver) = &self.early_receiver else {
            return false;
        };
        match receiver.try_recv() {
            Ok(info) => {
                self.early = Some(info);
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.early_receiver = None;
                false
            }
        }
    }
}

impl Panel for SnapshotPanel {
    fn label(&self) -> String {
        self.label.clone()
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
                self.poll_early();
                if self.draw_early(frame, area, focused, theme) {
                    return;
                }
                let title = panel_title("Diff");
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
            let early = self.poll_early();
            let announced = self.built.is_some();
            let ready = self.poll();
            return BackgroundWork {
                changed: (ready && !announced) || early,
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
