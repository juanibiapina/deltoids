//! Shared test fixtures for the `review` submodules: theme/file builders,
//! state constructors, event builders, and git repo helpers. Lives in one
//! place so each slice's `#[cfg(test)] mod tests` can pull what it needs.

use std::path::Path;

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use deltoids::Theme;
use deltoids::parse::FileDiff;

use crate::scroll::WheelScroll;
use crate::sidebar::{IconMode, Sidebar, SidebarFile, display_path};

use super::diff_pane::DiffPane;
use super::model::{Model, ResolvedFile, count_deltas, precompute_diffs};
use super::{FilesMode, Focus};

pub(super) fn theme() -> Theme {
    Theme::default()
}

/// Build a `FileDiff` with the given path. The `hunks` field is left
/// empty: the model runs `Diff::compute` against the supplied
/// before/after text, so the parsed hunks aren't read.
pub(super) fn file_diff(path: &str) -> FileDiff {
    FileDiff {
        preamble: Vec::new(),
        old_path: path.to_string(),
        new_path: path.to_string(),
        rename_from: None,
        old_hash: None,
        new_hash: None,
        hunks: Vec::new(),
    }
}

/// Concatenate the visible text of a `Line<'static>`.
pub(super) fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

pub(super) fn make_state(files: &[ResolvedFile]) -> FilesMode {
    let owned: Vec<ResolvedFile> = files.to_vec();
    let diffs = precompute_diffs(&owned);
    let model = Model {
        files: owned,
        diffs,
    };
    let sidebar_files: Vec<SidebarFile<'_>> = model
        .files
        .iter()
        .zip(model.diffs.iter())
        .map(|(f, d)| {
            let (added, deleted) = count_deltas(d);
            SidebarFile {
                file: &f.file,
                added,
                deleted,
            }
        })
        .collect();
    let sidebar = Sidebar::build_with_icons(&sidebar_files, &theme(), IconMode::Off);
    let display_order = sidebar.display_order();
    let diff = DiffPane::new(display_order, 80);
    FilesMode {
        diff,
        sidebar,
        focus: Focus::Sidebar,
        sidebar_rect: Rect::default(),
        diff_rect: Rect::default(),
        wheel: WheelScroll::new(),
        model,
        repo: None,
        is_static: true,
        last_input: String::new(),
        _watcher: None,
    }
}

pub(super) fn make_state_with_rects(files: &[ResolvedFile]) -> FilesMode {
    let mut state = make_state(files);
    state.sidebar_rect = Rect::new(0, 0, 38, 20);
    state.diff_rect = Rect::new(38, 0, 82, 20);
    state
}

/// A resolved file with distinct before/after so its diff is non-empty.
pub(super) fn resolved(path: &str) -> ResolvedFile {
    ResolvedFile {
        file: file_diff(path),
        before: format!("{path} old\n"),
        after: format!("{path} new\n"),
    }
}

pub(super) fn model_of(paths: &[&str]) -> Model {
    let files: Vec<ResolvedFile> = paths.iter().map(|p| resolved(p)).collect();
    let diffs = precompute_diffs(&files);
    Model { files, diffs }
}

/// Input index of the file owning the current sidebar selection.
pub(super) fn selected_path(state: &FilesMode, model: &Model) -> Option<String> {
    state
        .sidebar
        .nearest_file_index()
        .and_then(|i| model.files.get(i))
        .map(|f| display_path(&f.file).to_string())
}

pub(super) fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

pub(super) fn init_repo(dir: &Path) -> git2::Repository {
    let repo = git2::Repository::init(dir).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    repo
}

pub(super) fn stage_all(repo: &git2::Repository) {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
}

pub(super) fn commit_index(repo: &git2::Repository, msg: &str) {
    let mut index = repo.index().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = repo.signature().unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
        .unwrap();
}
