//! Symlink-change model: interpret a parsed [`FileDiff`] whose git file
//! mode is `120000` (a symbolic link) and reduce it to a small,
//! fully-decided view for the renderers.
//!
//! Git stores a symlink as a blob whose *content is the target path*
//! under file mode `120000`, so a plain `git diff` of a repointed link
//! is a one-line text hunk. Treating that as file content both misreads
//! the change and (for working-tree diffs) fails to resolve, because
//! reading the link follows it to the target file. This module sidesteps
//! all of that: it reads the old/new targets straight out of the hunk
//! lines and never touches the filesystem or the object database.
//!
//! The only public surface is [`SymlinkView`] plus
//! [`SymlinkView::from_file_diff`]. Mode interpretation, the action
//! taxonomy, hunk-line extraction, and the per-case wording are private:
//! callers ask `from_file_diff` and, on `Some`, hand the view to a
//! painter (see `render::render_symlink` / `render_tui::render_symlink`).

use crate::parse::{FileDiff, RawLineKind};

/// Git file mode for a symbolic link.
const SYMLINK_MODE: &str = "120000";

/// A fully-decided symlink change, ready to paint. Built by
/// [`SymlinkView::from_file_diff`]; the two renderers consume it without
/// re-deriving the action, the targets, or the wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkView {
    /// Human-readable action, e.g. `"symlink retargeted"`.
    pub description: String,
    /// The link target before the change (the removed hunk line), if any.
    pub old_target: Option<String>,
    /// The link target after the change (the added hunk line), if any.
    pub new_target: Option<String>,
}

/// Which symlink change this is. An internal detail of
/// [`SymlinkView::from_file_diff`]: callers never see it.
enum SymlinkAction {
    /// Link existed and now points somewhere else (`120000` both sides).
    Retargeted,
    /// Link created (`120000` on the new side only).
    Created,
    /// Link deleted (`120000` on the old side only).
    Deleted,
}

impl SymlinkAction {
    fn description(&self) -> &'static str {
        match self {
            SymlinkAction::Retargeted => "symlink retargeted",
            SymlinkAction::Created => "symlink created",
            SymlinkAction::Deleted => "symlink deleted",
        }
    }
}

impl SymlinkView {
    /// The only public entry. Returns `Some` iff `file` is a symlink
    /// change (either side has file mode `120000`). Interprets the modes,
    /// picks the description, and extracts the old/new targets from the
    /// hunk lines.
    pub fn from_file_diff(file: &FileDiff) -> Option<SymlinkView> {
        let old_is_link = file.old_mode.as_deref() == Some(SYMLINK_MODE);
        let new_is_link = file.new_mode.as_deref() == Some(SYMLINK_MODE);

        let action = match (old_is_link, new_is_link) {
            (true, true) => SymlinkAction::Retargeted,
            (false, true) => SymlinkAction::Created,
            (true, false) => SymlinkAction::Deleted,
            (false, false) => return None,
        };

        let (old_target, new_target) = extract_targets(file);

        Some(SymlinkView {
            description: action.description().to_string(),
            old_target,
            new_target,
        })
    }
}

/// Pull the old/new link targets out of the hunk lines. A symlink blob
/// is a single line whose text is the target path, so the removed line
/// is the old target and the added line is the new target.
fn extract_targets(file: &FileDiff) -> (Option<String>, Option<String>) {
    let mut old_target = None;
    let mut new_target = None;
    for hunk in &file.hunks {
        for line in &hunk.lines {
            match line.kind {
                RawLineKind::Removed => old_target = Some(line.content.clone()),
                RawLineKind::Added => new_target = Some(line.content.clone()),
                RawLineKind::Context => {}
            }
        }
    }
    (old_target, new_target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{RawHunk, RawLine};

    fn file_diff(old_mode: Option<&str>, new_mode: Option<&str>, lines: Vec<RawLine>) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: "link.txt".to_string(),
            new_path: "link.txt".to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            old_mode: old_mode.map(str::to_string),
            new_mode: new_mode.map(str::to_string),
            hunks: if lines.is_empty() {
                Vec::new()
            } else {
                vec![RawHunk {
                    old_start: 1,
                    old_count: 1,
                    new_start: 1,
                    new_count: 1,
                    lines,
                }]
            },
        }
    }

    fn removed(content: &str) -> RawLine {
        RawLine {
            kind: RawLineKind::Removed,
            content: content.to_string(),
        }
    }

    fn added(content: &str) -> RawLine {
        RawLine {
            kind: RawLineKind::Added,
            content: content.to_string(),
        }
    }

    #[test]
    fn non_symlink_returns_none() {
        let file = file_diff(Some("100644"), Some("100644"), vec![]);
        assert!(SymlinkView::from_file_diff(&file).is_none());
    }

    #[test]
    fn retargeted_reports_both_targets() {
        let file = file_diff(
            Some("120000"),
            Some("120000"),
            vec![removed("a.txt"), added("b.txt")],
        );
        let view = SymlinkView::from_file_diff(&file).expect("symlink view");
        assert_eq!(view.description, "symlink retargeted");
        assert_eq!(view.old_target, Some("a.txt".to_string()));
        assert_eq!(view.new_target, Some("b.txt".to_string()));
    }

    #[test]
    fn created_reports_new_target_only() {
        let file = file_diff(None, Some("120000"), vec![added("a.txt")]);
        let view = SymlinkView::from_file_diff(&file).expect("symlink view");
        assert_eq!(view.description, "symlink created");
        assert_eq!(view.old_target, None);
        assert_eq!(view.new_target, Some("a.txt".to_string()));
    }

    #[test]
    fn deleted_reports_old_target_only() {
        let file = file_diff(Some("120000"), None, vec![removed("a.txt")]);
        let view = SymlinkView::from_file_diff(&file).expect("symlink view");
        assert_eq!(view.description, "symlink deleted");
        assert_eq!(view.old_target, Some("a.txt".to_string()));
        assert_eq!(view.new_target, None);
    }

    #[test]
    fn file_to_symlink_entry_renders_as_created() {
        // Git splits a type change into a regular delete + a symlink
        // create; the create entry is a plain `symlink created`.
        let file = file_diff(None, Some("120000"), vec![added("a.txt")]);
        let view = SymlinkView::from_file_diff(&file).expect("symlink view");
        assert_eq!(view.description, "symlink created");
    }

    #[test]
    fn symlink_to_file_entry_renders_as_deleted() {
        let file = file_diff(Some("120000"), None, vec![removed("a.txt")]);
        let view = SymlinkView::from_file_diff(&file).expect("symlink view");
        assert_eq!(view.description, "symlink deleted");
    }
}
