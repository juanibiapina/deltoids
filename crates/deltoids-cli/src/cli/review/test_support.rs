//! Shared test fixtures for the `review` submodules: theme/file builders,
//! state constructors, event builders, and git repo helpers. Lives in one
//! place so each slice's `#[cfg(test)] mod tests` can pull what it needs.

use std::path::Path;

use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use deltoids::Theme;
use deltoids::parse::FileDiff;

use crate::sidebar::{IconMode, Sidebar, SidebarFile, display_path};
use crate::sidebar_width::Preference;

use super::ViewState;
use super::diff_pane::{DiffPane, build_view};
use super::model::{Model, ResolvedFile, count_deltas, precompute_diffs};

pub(super) fn theme() -> Theme {
    Theme::default()
}

/// Build a `FileDiff` with the given path. The `hunks` field is left
/// empty: `build_view` runs `Diff::compute` against the supplied
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

pub(super) fn make_state(files: &[ResolvedFile]) -> ViewState {
    let diffs = precompute_diffs(files);
    let sidebar_files: Vec<SidebarFile<'_>> = files
        .iter()
        .zip(diffs.iter())
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
    let view = build_view(files, &diffs, &display_order, 80, &theme());
    let diff = DiffPane::new(view, display_order, 80);
    ViewState::new(diff, sidebar, Preference::seeded(200))
}

pub(super) fn make_state_with_rects(files: &[ResolvedFile]) -> ViewState {
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
pub(super) fn selected_path(state: &ViewState, model: &Model) -> Option<String> {
    state
        .sidebar
        .nearest_file_index()
        .and_then(|i| model.files.get(i))
        .map(|f| display_path(&f.file).to_string())
}

pub(super) fn key_press(code: KeyCode) -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        code,
        crossterm::event::KeyModifiers::NONE,
    ))
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
