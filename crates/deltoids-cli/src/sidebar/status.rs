//! File classification axis: status (added/deleted/modified/renamed/
//! copied/type-changed), git file modes, and the metadata pulled from a
//! [`FileDiff`]'s preamble (binary flag, mode change, submodule flag).

use deltoids::parse::FileDiff;

/// One file's worth of input the sidebar needs to render.
///
/// A thin view over the data the rv main loop already has: the parsed
/// [`FileDiff`] (for paths, status, rename info) plus a count of added
/// and deleted lines (computed once from the resolved [`Diff`]).
#[derive(Debug, Clone)]
pub struct SidebarFile<'a> {
    pub file: &'a FileDiff,
    pub added: usize,
    pub deleted: usize,
    /// Two-column git staging status, mirroring `git status --porcelain`
    /// XY codes. `None` for piped diffs or when no repo is available, in
    /// which case the sidebar falls back to the single change-type letter
    /// derived from the combined diff.
    pub stage: Option<StageStatus>,
}

/// Sidebar-facing mirror of `deltoids::git::FileStageStatus`: the two
/// staging columns of a file. Kept as a sidebar type so this module
/// stays free of the `git2` dependency; `files/model.rs` maps the git
/// value into this one when building the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageStatus {
    /// Staged column (HEAD → index).
    pub staged: Option<ChangeKind>,
    /// Worktree column (index → workdir).
    pub unstaged: Option<ChangeKind>,
}

impl StageStatus {
    /// True when there are staged changes.
    pub fn is_staged(self) -> bool {
        self.staged.is_some()
    }

    /// True when there are worktree (unstaged) changes.
    pub fn is_unstaged(self) -> bool {
        self.unstaged.is_some()
    }
}

/// One change column's kind, mirroring `deltoids::git::StageChange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Untracked,
}

impl ChangeKind {
    /// Single-letter porcelain code for this change.
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::TypeChanged => 'T',
            ChangeKind::Untracked => '?',
        }
    }
}

/// Whether the sidebar treats the file as added, deleted, modified, or
/// renamed. Drives the colored single-letter status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    /// File copied from another location (`copy from`/`copy to` in the
    /// diff preamble). Lazygit shows this as `C`.
    Copied,
    /// File type changed: regular ↔ symlink ↔ submodule. Detected by
    /// comparing the leading mode digits of `old mode` / `new mode`.
    TypeChanged,
}

impl FileStatus {
    /// Single-letter badge displayed at the start of each file row.
    pub fn badge(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Modified => 'M',
            FileStatus::Renamed => 'R',
            FileStatus::Copied => 'C',
            FileStatus::TypeChanged => 'T',
        }
    }
}

/// Git file mode (`100644`, `100755`, `120000`, `160000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Regular file (`100644`).
    Regular,
    /// Executable file (`100755`).
    Executable,
    /// Symbolic link (`120000`).
    Symlink,
    /// Submodule / gitlink (`160000`).
    Submodule,
    /// Anything else (unrecognised octal).
    Other,
}

impl FileMode {
    /// Parse the six-octal-digit git mode (`"100644"`, etc.).
    pub fn parse(text: &str) -> Self {
        match text.trim() {
            "100644" => FileMode::Regular,
            "100755" => FileMode::Executable,
            "120000" => FileMode::Symlink,
            "160000" => FileMode::Submodule,
            _ => FileMode::Other,
        }
    }

    /// True when the mode change corresponds to flipping the
    /// executable bit on a regular file (rather than a real type
    /// change).
    fn is_regular_or_executable(self) -> bool {
        matches!(self, FileMode::Regular | FileMode::Executable)
    }
}

/// Mode change between the old and new versions of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChange {
    /// Executable bit set (regular → executable).
    ExecutableSet,
    /// Executable bit cleared (executable → regular).
    ExecutableCleared,
    /// Different file kinds either side of the change.
    TypeChange { old: FileMode, new: FileMode },
}

/// Extra metadata extracted from a [`FileDiff`]'s preamble: binary
/// flag, mode change, submodule flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileMetadata {
    pub binary: bool,
    pub mode_change: Option<ModeChange>,
    pub is_submodule: bool,
}

/// Classify a [`FileDiff`] as added / deleted / modified / renamed /
/// copied / type-changed.
///
/// - Added: old path is `/dev/null` or old hash is the null oid.
/// - Deleted: new path is `/dev/null` or new hash is the null oid.
/// - Copied: preamble has `copy from` / `copy to`.
/// - Renamed: `rename_from` is set and the file is neither added nor
///   deleted.
/// - TypeChanged: mode digits differ in the leading three octal
///   positions (regular ↔ symlink ↔ submodule).
/// - Modified: everything else.
pub fn file_status(file: &FileDiff) -> FileStatus {
    let old_absent = file.old_path == "/dev/null" || is_null_hash(file.old_hash.as_deref());
    let new_absent = file.new_path == "/dev/null" || is_null_hash(file.new_hash.as_deref());

    if old_absent && !new_absent {
        return FileStatus::Added;
    }
    if !old_absent && new_absent {
        return FileStatus::Deleted;
    }
    if preamble_has_prefix(&file.preamble, "copy from ") {
        return FileStatus::Copied;
    }
    if matches!(
        file_metadata(file).mode_change,
        Some(ModeChange::TypeChange { .. })
    ) {
        return FileStatus::TypeChanged;
    }
    if file.rename_from.is_some() {
        return FileStatus::Renamed;
    }
    FileStatus::Modified
}

/// Pull binary, mode-change, and submodule flags out of the preamble.
///
/// The preamble is the slice of non-diff lines that preceded the
/// `--- ` / `+++ ` markers. We look for:
///
/// - `Binary files ... and ... differ` → binary = true
/// - `old mode XXXXXX` / `new mode XXXXXX` → mode_change populated
///   (executable flip vs full type change)
/// - any mode equal to `160000` → is_submodule = true
pub fn file_metadata(file: &FileDiff) -> FileMetadata {
    let mut out = FileMetadata::default();
    let mut old_mode: Option<FileMode> = None;
    let mut new_mode: Option<FileMode> = None;

    for line in &file.preamble {
        let line = line.trim_start();
        if line.starts_with("Binary files ") && line.ends_with(" differ") {
            out.binary = true;
        } else if let Some(rest) = line.strip_prefix("old mode ") {
            old_mode = Some(FileMode::parse(rest));
        } else if let Some(rest) = line.strip_prefix("new mode ") {
            new_mode = Some(FileMode::parse(rest));
        } else if let Some(rest) = line.strip_prefix("new file mode ") {
            new_mode = Some(FileMode::parse(rest));
        } else if let Some(rest) = line.strip_prefix("deleted file mode ") {
            old_mode = Some(FileMode::parse(rest));
        }
    }

    if matches!(old_mode, Some(FileMode::Submodule))
        || matches!(new_mode, Some(FileMode::Submodule))
    {
        out.is_submodule = true;
    }

    if let (Some(o), Some(n)) = (old_mode, new_mode)
        && o != n
    {
        out.mode_change = Some(
            if o.is_regular_or_executable() && n.is_regular_or_executable() {
                match (o, n) {
                    (FileMode::Regular, FileMode::Executable) => ModeChange::ExecutableSet,
                    (FileMode::Executable, FileMode::Regular) => ModeChange::ExecutableCleared,
                    _ => ModeChange::TypeChange { old: o, new: n },
                }
            } else {
                ModeChange::TypeChange { old: o, new: n }
            },
        );
    }

    out
}

fn preamble_has_prefix(preamble: &[String], prefix: &str) -> bool {
    preamble
        .iter()
        .any(|line| line.trim_start().starts_with(prefix))
}

fn is_null_hash(hash: Option<&str>) -> bool {
    hash.is_some_and(|h| h.chars().all(|c| c == '0'))
}

/// Effective path for display: prefer `new_path`, fall back to
/// `old_path` for deletions (`new_path == "/dev/null"`).
pub fn display_path(file: &FileDiff) -> &str {
    if file.new_path == "/dev/null" {
        &file.old_path
    } else {
        &file.new_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::test_support::*;

    #[test]
    fn file_status_classifies_added_file() {
        assert_eq!(file_status(&fd_added("a.rs")), FileStatus::Added);
    }

    #[test]
    fn file_status_classifies_deleted_file() {
        assert_eq!(file_status(&fd_deleted("a.rs")), FileStatus::Deleted);
    }

    #[test]
    fn file_status_classifies_modified_file() {
        let mut f = fd("a.rs");
        f.old_hash = Some("a".repeat(40));
        f.new_hash = Some("b".repeat(40));
        assert_eq!(file_status(&f), FileStatus::Modified);
    }

    #[test]
    fn file_status_classifies_renamed_file() {
        assert_eq!(
            file_status(&fd_renamed("old.rs", "new.rs")),
            FileStatus::Renamed
        );
    }

    #[test]
    fn file_status_classifies_copied_file() {
        let f = fd_with_preamble(
            "new.rs",
            &[
                "diff --git a/old.rs b/new.rs",
                "similarity index 100%",
                "copy from old.rs",
                "copy to new.rs",
            ],
        );
        assert_eq!(file_status(&f), FileStatus::Copied);
    }

    #[test]
    fn file_status_classifies_type_changed_file() {
        // Old mode 100644 (regular) -> new mode 120000 (symlink).
        let f = fd_with_preamble(
            "link.txt",
            &[
                "diff --git a/link.txt b/link.txt",
                "old mode 100644",
                "new mode 120000",
            ],
        );
        assert_eq!(file_status(&f), FileStatus::TypeChanged);
    }

    #[test]
    fn file_metadata_detects_binary_marker() {
        let f = fd_with_preamble(
            "image.png",
            &[
                "diff --git a/image.png b/image.png",
                "index 9be8cca..cfe6e77 100644",
                "Binary files a/image.png and b/image.png differ",
            ],
        );
        let meta = file_metadata(&f);
        assert!(meta.binary, "expected binary flag, got {meta:?}");
    }

    #[test]
    fn file_metadata_detects_executable_set() {
        let f = fd_with_preamble(
            "script.sh",
            &[
                "diff --git a/script.sh b/script.sh",
                "old mode 100644",
                "new mode 100755",
            ],
        );
        let meta = file_metadata(&f);
        assert_eq!(meta.mode_change, Some(ModeChange::ExecutableSet));
    }

    #[test]
    fn file_metadata_detects_executable_cleared() {
        let f = fd_with_preamble("script.sh", &["old mode 100755", "new mode 100644"]);
        let meta = file_metadata(&f);
        assert_eq!(meta.mode_change, Some(ModeChange::ExecutableCleared));
    }

    #[test]
    fn file_metadata_detects_type_change() {
        let f = fd_with_preamble("link.txt", &["old mode 100644", "new mode 120000"]);
        let meta = file_metadata(&f);
        assert_eq!(
            meta.mode_change,
            Some(ModeChange::TypeChange {
                old: FileMode::Regular,
                new: FileMode::Symlink,
            })
        );
    }

    #[test]
    fn file_metadata_detects_submodule() {
        let f = fd_with_preamble(
            "vendor/lib",
            &[
                "diff --git a/vendor/lib b/vendor/lib",
                "index abc..def 160000",
                "new file mode 160000",
            ],
        );
        let meta = file_metadata(&f);
        assert!(meta.is_submodule);
    }
}
