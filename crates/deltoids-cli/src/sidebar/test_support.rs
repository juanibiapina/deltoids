//! Shared test fixtures for the sidebar submodules: a theme, `FileDiff`
//! builders for each status, and a line-text extractor.

use deltoids::Theme;
use deltoids::parse::FileDiff;
use ratatui::text::Line;

pub(super) fn theme() -> Theme {
    Theme::default()
}

pub(super) fn fd(path: &str) -> FileDiff {
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

pub(super) fn fd_added(path: &str) -> FileDiff {
    FileDiff {
        preamble: Vec::new(),
        old_path: "/dev/null".to_string(),
        new_path: path.to_string(),
        rename_from: None,
        old_hash: Some("0".repeat(40)),
        new_hash: Some("a".repeat(40)),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
    }
}

pub(super) fn fd_deleted(path: &str) -> FileDiff {
    FileDiff {
        preamble: Vec::new(),
        old_path: path.to_string(),
        new_path: "/dev/null".to_string(),
        rename_from: None,
        old_hash: Some("a".repeat(40)),
        new_hash: Some("0".repeat(40)),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
    }
}

pub(super) fn fd_renamed(old: &str, new: &str) -> FileDiff {
    FileDiff {
        preamble: Vec::new(),
        old_path: old.to_string(),
        new_path: new.to_string(),
        rename_from: Some(old.to_string()),
        old_hash: Some("a".repeat(40)),
        new_hash: Some("b".repeat(40)),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
    }
}

pub(super) fn fd_with_preamble(path: &str, preamble: &[&str]) -> FileDiff {
    let mut f = fd(path);
    f.preamble = preamble.iter().map(|s| s.to_string()).collect();
    f.old_hash = Some("a".repeat(40));
    f.new_hash = Some("b".repeat(40));
    f
}

pub(super) fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
