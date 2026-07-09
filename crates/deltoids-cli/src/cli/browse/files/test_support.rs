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
use super::model::{Model, ResolvedFile, body_deltas, precompute_bodies};
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
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
    }
}

/// Concatenate the visible text of a `Line<'static>`.
pub(super) fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

pub(super) fn make_state(files: &[ResolvedFile]) -> FilesMode {
    let owned: Vec<ResolvedFile> = files.to_vec();
    let bodies = precompute_bodies(&owned);
    let model = Model {
        files: owned,
        bodies,
        stages: Default::default(),
    };
    let sidebar_files: Vec<SidebarFile<'_>> = model
        .files
        .iter()
        .zip(model.bodies.iter())
        .map(|(f, b)| {
            let (added, deleted) = body_deltas(b);
            SidebarFile {
                file: &f.file,
                added,
                deleted,
                stage: None,
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

/// A resolved binary file: its preamble carries the `Binary files ...
/// differ` marker so `file_metadata` classifies it as binary, with empty
/// before/after (content resolution is skipped for binaries).
pub(super) fn binary_resolved(path: &str) -> ResolvedFile {
    let mut file = file_diff(path);
    file.preamble = vec![format!("Binary files a/{path} and b/{path} differ")];
    ResolvedFile {
        file,
        before: String::new(),
        after: String::new(),
    }
}

/// A resolved submodule commit bump: gitlink mode `160000` on both sides
/// with distinct commit hashes, empty before/after (content resolution is
/// skipped for submodules).
pub(super) fn submodule_resolved(path: &str, old: &str, new: &str) -> ResolvedFile {
    let mut file = file_diff(path);
    file.old_mode = Some("160000".to_string());
    file.new_mode = Some("160000".to_string());
    file.old_hash = Some(old.to_string());
    file.new_hash = Some(new.to_string());
    ResolvedFile {
        file,
        before: String::new(),
        after: String::new(),
    }
}

/// A resolved regular→submodule type change: a preamble `old mode 100644`
/// / `new mode 160000` so `file_metadata` reports both a type change and
/// the submodule flag.
pub(super) fn submodule_typechange_resolved(path: &str, new: &str) -> ResolvedFile {
    let mut file = file_diff(path);
    file.preamble = vec!["old mode 100644".to_string(), "new mode 160000".to_string()];
    file.old_mode = Some("100644".to_string());
    file.new_mode = Some("160000".to_string());
    file.new_hash = Some(new.to_string());
    ResolvedFile {
        file,
        before: String::new(),
        after: String::new(),
    }
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
    let bodies = precompute_bodies(&files);
    Model {
        files,
        bodies,
        stages: Default::default(),
    }
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
    make_mouse_mods(kind, col, row, crossterm::event::KeyModifiers::NONE)
}

pub(super) fn make_mouse_mods(
    kind: MouseEventKind,
    col: u16,
    row: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers,
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

/// Run `git` in `dir` via the CLI, asserting success. Submodule
/// operations must run through the git CLI (libgit2 cannot `submodule
/// add`), so submodule fixtures shell out here.
pub(super) fn git_cli(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Build an outer repo at `dir` containing a submodule `sub` pinned at an
/// older commit, then bump the working-tree checkout to a newer commit so
/// the outer tree shows an unstaged submodule commit bump. Uses a nested
/// source repo (built under `dir/../sub_src`) with two commits. Returns
/// the outer repo path (`dir`).
pub(super) fn setup_submodule_bump(dir: &Path) {
    let src = dir.parent().unwrap().join("sub_src");
    std::fs::create_dir_all(&src).unwrap();
    git_cli(&src, &["init", "-q"]);
    std::fs::write(src.join("file.txt"), "v1\n").unwrap();
    git_cli(&src, &["add", "-A"]);
    git_cli(&src, &["commit", "-qm", "sub v1"]);
    let v1 = String::from_utf8(git_cli(&src, &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    std::fs::write(src.join("file.txt"), "v2\n").unwrap();
    git_cli(&src, &["add", "-A"]);
    git_cli(&src, &["commit", "-qm", "sub v2"]);

    git_cli(dir, &["init", "-q"]);
    std::fs::write(dir.join("readme.txt"), "hello\n").unwrap();
    git_cli(dir, &["add", "-A"]);
    git_cli(dir, &["commit", "-qm", "init"]);
    git_cli(
        dir,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            src.to_str().unwrap(),
            "sub",
        ],
    );
    // Pin the submodule to v1 and commit that pointer.
    git_cli(&dir.join("sub"), &["checkout", "-q", &v1]);
    git_cli(dir, &["add", "sub"]);
    git_cli(dir, &["commit", "-qm", "add submodule at v1"]);
    // Bump the working-tree checkout to v2: an unstaged commit bump.
    git_cli(&dir.join("sub"), &["checkout", "-q", "-"]);
}
