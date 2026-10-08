//! Lazygit-inspired left sidebar for `rv`: a scrollable file tree with
//! status badges, nerd-font icons, and per-file line-delta counts.
//!
//! Public entry points:
//!
//! - [`Sidebar::build`] — construct from a slice of [`SidebarFile`]s.
//! - [`Sidebar::rows`] — pre-styled rows ready to render in a
//!   [`ratatui::widgets::Paragraph`].
//! - [`Sidebar::selected_file_index`] — index into the original file
//!   slice, or `None` when a directory row is selected.
//! - [`Sidebar::move_up`] / [`Sidebar::move_down`] / [`Sidebar::top`] /
//!   [`Sidebar::bottom`] / [`Sidebar::page_up`] / [`Sidebar::page_down`]
//!   — navigation over visible rows (files and directories).
//! - [`Sidebar::toggle_selected_dir`] — fold or unfold the selected
//!   directory; [`Sidebar::folds`] / [`Sidebar::apply_folds`] carry
//!   folds into a rebuilt sidebar.
//! - [`Sidebar::scroll`] — current scroll offset (auto-tracked to keep
//!   selection visible).
//! - [`Sidebar::row_count`] — visible rows (rows under a folded
//!   directory are hidden).
//!
//! ## Module layout
//!
//! Split by concern:
//!
//! - [`status`] — file classification (status, modes, preamble metadata)
//!   and [`display_path`].
//! - [`tree`] — path-tree construction and the [`Row`] list.
//! - [`icons`] — nerd-font glyph tables and [`IconMode`].
//! - [`render`] — turning a row into a styled line.
//!
//! This module owns the [`Sidebar`] state itself: selection, scroll, the
//! cached rendered lines, and the navigation/selection-range interface.

use std::collections::BTreeSet;
use std::ops::Range;

use deltoids::Theme;
use ratatui::text::Line;

mod icons;
mod render;
mod status;
#[cfg(test)]
mod test_support;
mod tree;

pub use icons::{IconMode, file_icon, symlink_icon};
pub use status::{
    ChangeKind, DirStage, FileMetadata, FileMode, FileStatus, ModeChange, SidebarFile, StageStatus,
    display_path, file_metadata, file_status, is_binary_marker,
};

use render::{copy_origin, rename_leaf, render_row};
use tree::{Row, build_rows};

/// Aggregate statistics about the resolved file set, summarised for
/// display below the sidebar (total file count plus cumulative line
/// deltas across all files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub files: usize,
    pub added: usize,
    pub deleted: usize,
}

/// Pre-rendered rows + selection + scroll. Driven by the rv main loop:
/// build once per resolved file set, then call `move_*` and `rows()`
/// each frame.
///
/// Positions in the interface (`selected`, `scroll`, `row_count`,
/// `rows`, `set_selected`) count visible rows only: rows under a folded
/// directory are skipped. Subtree queries (`selection_display_range`,
/// `display_order`, `nearest_file_index`) still see every file.
#[derive(Debug)]
pub struct Sidebar {
    /// Every row of the tree, folded or not.
    rows: Vec<Row>,
    /// Workdir-relative path of each directory row; `None` for files.
    dir_paths: Vec<Option<String>>,
    /// Row indices currently shown, in order.
    visible: Vec<usize>,
    /// Paths of folded directories.
    folds: Folds,
    /// Visible position of the highlighted row.
    selected: usize,
    /// First visible position in the viewport.
    scroll: usize,
    /// One cached line per visible row, regenerated whenever `selected`,
    /// the folds, or the staging data change.
    rendered: Vec<Line<'static>>,
    /// Captured at build time so tests can supply specific values.
    icons: IconMode,
    /// Per-row display data, indexed like `rows`.
    file_meta: Vec<FileRowMeta>,
    /// Theme stored so re-rendering on selection change is self-contained.
    theme: Theme,
    /// Aggregate statistics computed at build time.
    totals: Totals,
}

/// The set of folded directories, carried from one sidebar to the next
/// when the file set is rebuilt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds(BTreeSet<String>);

#[derive(Debug, Clone)]
pub(super) struct FileRowMeta {
    /// `None` for directory rows.
    pub(super) status: Option<FileStatus>,
    /// Two-column git staging status. `None` for directory rows and for
    /// files outside a repo, where the single-letter `status`
    /// badge is used instead.
    pub(super) stage: Option<StageStatus>,
    /// Aggregate staging state across a directory's subtree. `Some` only
    /// for directory rows that have stage data; `None` for file rows and
    /// for directories with no stage data (no repo), which
    /// keep the muted directory styling.
    pub(super) dir_stage: Option<DirStage>,
    /// `None` for directory rows.
    pub(super) deltas: Option<(usize, usize)>,
    /// `Some((old, new))` for renamed *and* copied files; the row
    /// label shows `old → new` instead of just `new`.
    pub(super) rename: Option<(String, String)>,
    /// Binary / mode-change / submodule flags pulled from the diff
    /// preamble. Empty for directory rows.
    pub(super) extra: FileMetadata,
}

impl Sidebar {
    /// Build the sidebar from a slice of files plus a theme. Captures the
    /// icon mode from the environment.
    pub fn build(files: &[SidebarFile<'_>], theme: &Theme) -> Self {
        Self::build_with_icons(files, theme, IconMode::from_env())
    }

    /// Same as `build` but with an explicit icon mode (for tests).
    pub fn build_with_icons(files: &[SidebarFile<'_>], theme: &Theme, icons: IconMode) -> Self {
        let rows = build_rows(files);
        let stages: Vec<_> = files.iter().map(|file| file.stage).collect();
        let file_meta = rows
            .iter()
            .enumerate()
            .map(|(idx, row)| match row {
                Row::Dir { .. } => FileRowMeta {
                    status: None,
                    stage: None,
                    dir_stage: dir_aggregate(&rows, &stages, idx),
                    deltas: None,
                    rename: None,
                    extra: FileMetadata::default(),
                },
                Row::File { file_index, .. } => {
                    let f = &files[*file_index];
                    let status = file_status(f.file);
                    let rename = match status {
                        FileStatus::Renamed => f
                            .file
                            .rename_from
                            .as_ref()
                            .map(|old| (rename_leaf(old), rename_leaf(&f.file.new_path))),
                        FileStatus::Copied => copy_origin(f.file)
                            .map(|old| (rename_leaf(&old), rename_leaf(&f.file.new_path))),
                        _ => None,
                    };
                    FileRowMeta {
                        status: Some(status),
                        stage: f.stage,
                        dir_stage: None,
                        deltas: Some((f.added, f.deleted)),
                        rename,
                        extra: file_metadata(f.file),
                    }
                }
            })
            .collect::<Vec<_>>();

        let selected = rows
            .iter()
            .position(|row| matches!(row, Row::File { .. }))
            .unwrap_or(0);

        let totals = Totals {
            files: files.len(),
            added: files.iter().map(|f| f.added).sum(),
            deleted: files.iter().map(|f| f.deleted).sum(),
        };

        let mut sidebar = Self {
            dir_paths: dir_paths(&rows),
            visible: (0..rows.len()).collect(),
            rows,
            folds: Folds::default(),
            selected,
            scroll: 0,
            rendered: Vec::new(),
            icons,
            file_meta,
            theme: theme.clone(),
            totals,
        };
        sidebar.render_all();
        sidebar
    }

    /// Update staging columns without rebuilding rows or moving selection and scroll.
    pub fn update_staging(&mut self, stages: &[Option<StageStatus>]) {
        for (index, row) in self.rows.iter().enumerate() {
            match row {
                Row::File { file_index, .. } => self.file_meta[index].stage = stages[*file_index],
                Row::Dir { .. } => {
                    self.file_meta[index].dir_stage = dir_aggregate(&self.rows, stages, index)
                }
            }
        }
        self.render_all();
    }

    /// File count + aggregate `+`/`-` totals, computed at build time.
    pub fn totals(&self) -> Totals {
        self.totals
    }

    /// Number of visible rows (dirs + files outside folded directories).
    pub fn row_count(&self) -> usize {
        self.visible.len()
    }

    /// File indices (into the original `&[SidebarFile]` slice) in the
    /// order they appear in the tree, including files inside folded
    /// directories.
    ///
    /// Used by the rv main loop to render diff content in tree order so
    /// the diff pane's vertical layout always matches the sidebar.
    pub fn display_order(&self) -> Vec<usize> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::File { file_index, .. } => Some(*file_index),
                _ => None,
            })
            .collect()
    }

    /// Pre-styled visible rows ready for a `Paragraph`. Borrowed; owned
    /// by the sidebar.
    pub fn rows(&self) -> &[Line<'static>] {
        &self.rendered
    }

    /// Currently-selected visible position.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Index into `rows` of the selected row.
    fn selected_row(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    /// Index into the original `&[SidebarFile]` slice for the selected
    /// row, or `None` when a directory row is selected.
    ///
    /// Production code uses [`Sidebar::nearest_file_index`] instead, so
    /// the diff pane stays in sync as the user traverses dirs; this
    /// stricter accessor is kept for tests and for callers that want to
    /// distinguish "a file is selected" from "a directory is selected".
    #[allow(dead_code)]
    pub fn selected_file_index(&self) -> Option<usize> {
        match self.rows.get(self.selected_row()?) {
            Some(Row::File { file_index, .. }) => Some(*file_index),
            _ => None,
        }
    }

    /// First visible position in the viewport. Auto-tracked by the move
    /// methods so the selection always stays in view.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether the currently-selected row is a directory header.
    pub fn selected_is_dir(&self) -> bool {
        self.selected_row()
            .is_some_and(|row| matches!(self.rows[row], Row::Dir { .. }))
    }

    /// Workdir-relative identity of the selected directory, including collapsed chains.
    pub fn selected_directory_path(&self) -> Option<String> {
        self.dir_paths.get(self.selected_row()?)?.clone()
    }

    /// Restore a directory selection after rebuilding the tree. A
    /// directory inside a folded one selects the folded ancestor.
    pub fn select_directory_path(&mut self, path: &str, viewport: usize) -> bool {
        let Some(row) = self
            .dir_paths
            .iter()
            .position(|dir| dir.as_deref() == Some(path))
        else {
            return false;
        };
        self.set_selected(self.position_of_row(row), viewport);
        true
    }

    /// Fold the selected directory, or unfold it when already folded.
    /// Returns `false` (and does nothing) on a file row.
    pub fn toggle_selected_dir(&mut self, viewport: usize) -> bool {
        let Some(row) = self.selected_row() else {
            return false;
        };
        let Some(path) = self.dir_paths[row].clone() else {
            return false;
        };
        if !self.folds.0.remove(&path) {
            self.folds.0.insert(path);
        }
        self.refold(row, viewport);
        true
    }

    /// The folded directories, to carry into a rebuilt sidebar.
    pub fn folds(&self) -> Folds {
        self.folds.clone()
    }

    /// Fold the directories in `folds` that exist in this tree. Keeps
    /// the selected row, or its folded ancestor when it becomes hidden.
    pub fn apply_folds(&mut self, folds: Folds, viewport: usize) {
        let mut paths = folds.0;
        paths.retain(|path| self.dir_paths.iter().flatten().any(|dir| dir == path));
        self.folds = Folds(paths);
        let row = self.selected_row().unwrap_or(0);
        self.refold(row, viewport);
    }

    /// Recompute visibility after the folds changed, keeping `row` (or
    /// its folded ancestor) selected.
    fn refold(&mut self, row: usize, viewport: usize) {
        let mut visible = Vec::with_capacity(self.rows.len());
        let mut folded_depth: Option<usize> = None;
        for (index, candidate) in self.rows.iter().enumerate() {
            let depth = row_depth(candidate);
            if folded_depth.is_some_and(|folded| depth > folded) {
                continue;
            }
            folded_depth = None;
            visible.push(index);
            if self.is_folded(index) {
                folded_depth = Some(depth);
            }
        }
        self.visible = visible;
        self.selected = self.position_of_row(row);
        self.scroll = self.scroll.min(self.selected);
        self.adjust_scroll(viewport);
        self.render_all();
    }

    fn is_folded(&self, row: usize) -> bool {
        self.dir_paths[row]
            .as_ref()
            .is_some_and(|path| self.folds.0.contains(path))
    }

    /// Visible position of `row`, or of the folded directory that hides it.
    fn position_of_row(&self, row: usize) -> usize {
        self.visible
            .partition_point(|&index| index <= row)
            .saturating_sub(1)
    }

    /// Input index of the file the diff pane should snap to for the
    /// current selection.
    ///
    /// On a file row this is just the selected file. On a directory
    /// row it's the first file inside that directory's subtree (the
    /// next file row at or after the selection — since each directory
    /// header is followed by its contents). `None` only when there are
    /// no files at all.
    pub fn nearest_file_index(&self) -> Option<usize> {
        self.rows
            .iter()
            .skip(self.selected_row()?)
            .find_map(|r| match r {
                Row::File { file_index, .. } => Some(*file_index),
                Row::Dir { .. } => None,
            })
    }

    /// Display-order positions of files matching the current selection.
    ///
    /// - File row: a single-element range covering just that file.
    /// - Directory row: the contiguous range of every file inside the
    ///   subtree, folded or not.
    /// - No files at all: `None`.
    ///
    /// Either way, the renderer can slice the diff lines between the
    /// range's first file offset and the line just before the next
    /// file's separator. This is the single source of truth for the
    /// diff pane's filter.
    pub fn selection_display_range(&self) -> Option<Range<usize>> {
        let row = self.selected_row()?;
        match self.rows[row] {
            Row::Dir { depth, .. } => self.subtree_range_for_dir(row, depth),
            Row::File { .. } => {
                let start = self.files_before(row);
                Some(start..start + 1)
            }
        }
    }

    /// Display-order range covering the subtree files of the directory
    /// at `dir_row`.
    ///
    /// Walks rows after the directory header until it hits one at or
    /// above the parent's depth. Returns `None` when the subtree is
    /// empty (defensive — directories are only emitted when they
    /// contain files).
    fn subtree_range_for_dir(&self, dir_row: usize, parent_depth: usize) -> Option<Range<usize>> {
        let start = self.files_before(dir_row);
        let count = self.rows[dir_row + 1..]
            .iter()
            .take_while(|row| row_depth(row) > parent_depth)
            .filter(|row| matches!(row, Row::File { .. }))
            .count();
        (count > 0).then_some(start..start + count)
    }

    /// Count of file rows strictly before `row_idx`. Equivalent to
    /// the file's position in display order when `row_idx` is itself
    /// a file row.
    fn files_before(&self, row_idx: usize) -> usize {
        self.rows[..row_idx]
            .iter()
            .filter(|r| matches!(r, Row::File { .. }))
            .count()
    }

    /// Move to the next visible row (file or directory).
    pub fn move_down(&mut self, viewport: usize) {
        if self.selected + 1 < self.visible.len() {
            self.set_selected(self.selected + 1, viewport);
        }
    }

    /// Move to the previous visible row (file or directory).
    pub fn move_up(&mut self, viewport: usize) {
        if self.selected > 0 {
            self.set_selected(self.selected - 1, viewport);
        }
    }

    /// Jump to the first row.
    pub fn top(&mut self, viewport: usize) {
        if !self.visible.is_empty() {
            self.set_selected(0, viewport);
        }
    }

    /// Jump to the last visible row.
    pub fn bottom(&mut self, viewport: usize) {
        if let Some(last) = self.visible.len().checked_sub(1) {
            self.set_selected(last, viewport);
        }
    }

    /// Move down by `viewport` rows, clamped at the last visible row.
    pub fn page_down(&mut self, viewport: usize) {
        let target = self
            .selected
            .saturating_add(viewport.max(1))
            .min(self.visible.len().saturating_sub(1));
        self.set_selected(target, viewport);
    }

    /// Move up by `viewport` rows, clamped at the first row.
    pub fn page_up(&mut self, viewport: usize) {
        let target = self.selected.saturating_sub(viewport.max(1));
        self.set_selected(target, viewport);
    }

    /// Select the row for the file with input index `file_index`, or the
    /// folded directory that hides it. Returns `true` when a matching
    /// file row was found. No-op (returns `false`) when no row owns that
    /// file index.
    pub fn select_file_index(&mut self, file_index: usize, viewport: usize) -> bool {
        let row = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::File { file_index: fi, .. } if *fi == file_index));
        match row {
            Some(row) => {
                self.set_selected(self.position_of_row(row), viewport);
                true
            }
            None => false,
        }
    }

    /// Select the visible row at position `target`.
    pub fn set_selected(&mut self, target: usize, viewport: usize) {
        if target == self.selected {
            return;
        }
        let previous = self.selected;
        self.selected = target;
        self.adjust_scroll(viewport);
        for position in [previous, target] {
            if position < self.visible.len() {
                self.rendered[position] = self.render_position(position);
            }
        }
    }

    /// Keep the selected row inside `[scroll, scroll + viewport)`.
    fn adjust_scroll(&mut self, viewport: usize) {
        let viewport = viewport.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport {
            self.scroll = self.selected + 1 - viewport;
        }
    }

    fn render_position(&self, position: usize) -> Line<'static> {
        let index = self.visible[position];
        render_row(
            &self.rows[index],
            &self.file_meta[index],
            position == self.selected,
            self.is_folded(index),
            self.icons,
            &self.theme,
        )
    }

    fn render_all(&mut self) {
        self.rendered = (0..self.visible.len())
            .map(|position| self.render_position(position))
            .collect();
    }
}

fn row_depth(row: &Row) -> usize {
    match row {
        Row::Dir { depth, .. } | Row::File { depth, .. } => *depth,
    }
}

/// Workdir-relative path of every directory row (the concatenated
/// labels of its ancestors and itself); `None` for file rows.
fn dir_paths(rows: &[Row]) -> Vec<Option<String>> {
    let mut parents: Vec<&str> = Vec::new();
    rows.iter()
        .map(|row| match row {
            Row::Dir { label, depth } => {
                parents.truncate(*depth);
                parents.push(label);
                Some(parents.concat())
            }
            Row::File { .. } => None,
        })
        .collect()
}

/// Fold the staging state of every file beneath the directory row at
/// `dir_idx` into a single [`DirStage`], mirroring lazygit's per-node
/// `hasStagedChanges` / `hasUnstagedChanges`. Walks rows after the
/// header while their depth stays greater than the directory's, so it
/// covers the whole subtree (nested dirs included). Returns `None` when
/// no subtree file carries stage data (no repo), which
/// keeps the directory's muted styling.
fn dir_aggregate(rows: &[Row], stages: &[Option<StageStatus>], dir_idx: usize) -> Option<DirStage> {
    let Row::Dir {
        depth: dir_depth, ..
    } = &rows[dir_idx]
    else {
        return None;
    };
    let mut any = false;
    let mut has_staged = false;
    let mut has_unstaged = false;
    for row in &rows[dir_idx + 1..] {
        let (row_depth, file_index) = match row {
            Row::Dir { depth, .. } => (*depth, None),
            Row::File {
                depth, file_index, ..
            } => (*depth, Some(*file_index)),
        };
        if row_depth <= *dir_depth {
            break;
        }
        if let Some(file_index) = file_index
            && let Some(stage) = stages[file_index]
        {
            any = true;
            has_staged |= stage.is_staged();
            has_unstaged |= stage.is_unstaged();
        }
    }
    any.then_some(DirStage {
        has_staged,
        has_unstaged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::test_support::*;
    use deltoids::parse::FileDiff;

    #[test]
    fn navigation_highlights_only_the_selected_row() {
        let diffs = [fd("a/one.rs"), fd("a/two.rs"), fd("b/three.rs")];
        let files: Vec<_> = diffs
            .iter()
            .map(|file| SidebarFile {
                file,
                added: 2,
                deleted: 1,
                stage: None,
            })
            .collect();
        let theme = theme();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme, IconMode::Off);
        for target in [sidebar.row_count() - 1, 0, 2, 1] {
            sidebar.set_selected(target, 2);
            for (index, line) in sidebar.rows().iter().enumerate() {
                let expected = (index == target)
                    .then(|| deltoids::render_tui::rgb_to_color(theme.selection_bg));
                assert!(line.spans.iter().all(|span| span.style.bg == expected));
            }
        }
    }

    #[test]
    fn staging_refresh_matches_a_fresh_sidebar_and_preserves_navigation_and_totals() {
        let diffs = [fd("a/src/one.rs"), fd("a/tests/two.rs"), fd("b/three.rs")];
        let unstaged = Some(StageStatus {
            staged: None,
            unstaged: Some(ChangeKind::Modified),
        });
        let mut files: Vec<_> = diffs
            .iter()
            .map(|file| SidebarFile {
                file,
                added: 7,
                deleted: 3,
                stage: unstaged,
            })
            .collect();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert!(sidebar.select_directory_path("a/", 2));
        let position = (sidebar.selected(), sidebar.scroll());
        let totals = sidebar.totals();
        files[0].stage = Some(StageStatus {
            staged: Some(ChangeKind::Modified),
            unstaged: None,
        });
        sidebar.update_staging(&files.iter().map(|file| file.stage).collect::<Vec<_>>());
        let mut expected = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert!(expected.select_directory_path("a/", 2));
        assert_eq!(sidebar.rows(), expected.rows());
        assert_eq!((sidebar.selected(), sidebar.scroll()), position);
        assert_eq!(sidebar.totals(), totals);
    }

    #[test]
    fn build_selects_first_file_skipping_dir_header() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // rows: src/ , a.rs — selected should be 1
        assert_eq!(sidebar.selected(), 1);
        assert_eq!(sidebar.selected_file_index(), Some(0));
    }

    #[test]
    fn move_down_advances_one_row_including_dirs() {
        // Layout:
        //   crates/                      row 0 dir
        //     deltoids/src/              row 1 dir
        //       lib.rs                   row 2 file
        //     deltoids-cli/src/          row 3 dir
        //       lib.rs                   row 4 file
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/lib.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Initial selection still lands on the first file row, so the
        // diff has something useful to snap to on startup.
        let first = sidebar.selected();
        assert!(
            !sidebar.selected_is_dir(),
            "initial selection must be a file row"
        );
        // From the first file, moving up walks back into directory
        // headers one row at a time.
        sidebar.move_up(20);
        assert_eq!(sidebar.selected(), first - 1);
        assert!(
            sidebar.selected_is_dir(),
            "move_up from a file should land on its parent dir row"
        );
        // Moving down again returns to the file.
        sidebar.move_down(20);
        assert_eq!(sidebar.selected(), first);
        assert!(!sidebar.selected_is_dir());
    }

    #[test]
    fn move_down_at_last_file_is_noop() {
        let a = fd("a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let before = sidebar.selected();
        sidebar.move_down(20);
        assert_eq!(sidebar.selected(), before);
    }

    #[test]
    fn move_up_with_no_dirs_above_is_noop() {
        // Top-level file: there's no row above row 0, so move_up has
        // nowhere to go.
        let a = fd("a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let before = sidebar.selected();
        sidebar.move_up(20);
        assert_eq!(sidebar.selected(), before);
    }

    #[test]
    fn top_jumps_to_first_row_and_bottom_jumps_to_last() {
        let a = fd("a/x.rs");
        let b = fd("b/y.rs");
        let c = fd("c/z.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.bottom(20);
        // Last row is the last file (c/z.rs file), since each dir is
        // followed by its single file leaf.
        assert_eq!(sidebar.selected(), sidebar.row_count() - 1);
        assert!(!sidebar.selected_is_dir());
        sidebar.top(20);
        // First row is the first directory header.
        assert_eq!(sidebar.selected(), 0);
        assert!(sidebar.selected_is_dir());
    }

    #[test]
    fn nearest_file_index_on_dir_returns_first_file_in_subtree() {
        // Layout: src/{a.rs,b.rs}, top-level z.rs.
        // Rows (mixed): src/ ; src/a.rs ; src/b.rs ; z.rs.
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let c = fd("z.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on the src/ header.
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        // nearest_file_index points at src/a.rs (input index 0).
        assert_eq!(sidebar.nearest_file_index(), Some(0));
        // Move down to a.rs; nearest is itself.
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(0));
        // Then b.rs.
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(1));
        // Then z.rs (top-level file).
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(2));
    }

    #[test]
    fn selected_file_index_returns_none_on_dir_row() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.selected_file_index(), None);
        // nearest_file_index still finds the file under it.
        assert_eq!(sidebar.nearest_file_index(), Some(0));
    }

    #[test]
    fn selection_range_for_file_is_single_element() {
        // src/a.rs only; initial selection is the file row.
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert!(!sidebar.selected_is_dir());
        // The file's display position is 0 — the only file.
        assert_eq!(sidebar.selection_display_range(), Some(0..1));
    }

    #[test]
    fn selection_range_dir_covers_subtree_files() {
        // Layout:
        //   src/                     (dir 0)
        //     a.rs                   (file 0)
        //     b.rs                   (file 1)
        //   util/                    (dir 2)
        //     c.rs                   (file 2)
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let c = fd("util/c.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on src/.
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.selection_display_range(), Some(0..2));

        // Step onto a.rs — single-element range.
        sidebar.move_down(20);
        assert!(!sidebar.selected_is_dir());
        assert_eq!(sidebar.selection_display_range(), Some(0..1));

        // Step onto b.rs — single-element range with shifted start.
        sidebar.move_down(20);
        assert_eq!(sidebar.selection_display_range(), Some(1..2));

        // Land on util/ (dir 2 in row order, after src/, a.rs, b.rs).
        sidebar.move_down(20); // util/
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.selection_display_range(), Some(2..3));
    }

    #[test]
    fn selection_range_dir_covers_nested_subtrees() {
        // Layout (from earlier example, multiple subdirs under crates/):
        //   crates/                          depth 0
        //     deltoids/                      depth 1
        //       src/                         depth 2
        //         lib.rs                     depth 3 (file 0 in display)
        //     deltoids-cli/                  depth 1
        //       src/                         depth 2
        //         lib.rs                     depth 3 (file 1 in display)
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/lib.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on crates/ (row 0). Subtree includes both files.
        sidebar.top(20);
        assert_eq!(sidebar.selection_display_range(), Some(0..2));

        // Move to deltoids/ (depth 1). Subtree includes only lib.rs (file 0).
        sidebar.move_down(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.selection_display_range(), Some(0..1));
    }

    #[test]
    fn page_down_advances_by_viewport_rows() {
        // 8 top-level files, viewport 3. From the initial selection
        // (row 0), page_down(3) should land on row 3, then row 6, then
        // clamp at the last row (7).
        let owned: Vec<FileDiff> = (0..8).map(|i| fd(&format!("f{i}.rs"))).collect();
        let files: Vec<_> = owned
            .iter()
            .map(|f| SidebarFile {
                file: f,
                added: 0,
                deleted: 0,
                stage: None,
            })
            .collect();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert_eq!(sidebar.selected(), 0);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 3);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 6);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 7);
        sidebar.page_up(3);
        assert_eq!(sidebar.selected(), 4);
    }

    #[test]
    fn scroll_keeps_selection_in_view_on_move_down() {
        // 8 files, viewport 3.  Moving down past row 3 should bump scroll.
        let owned: Vec<FileDiff> = (0..8).map(|i| fd(&format!("f{i}.rs"))).collect();
        let files: Vec<_> = owned
            .iter()
            .map(|f| SidebarFile {
                file: f,
                added: 0,
                deleted: 0,
                stage: None,
            })
            .collect();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert_eq!(sidebar.scroll(), 0);
        for _ in 0..7 {
            sidebar.move_down(3);
        }
        // Selected must be visible: scroll <= selected < scroll + 3.
        let s = sidebar.selected();
        let scroll = sidebar.scroll();
        assert!(
            scroll <= s && s < scroll + 3,
            "selection {s} not in viewport [{scroll}, {})",
            scroll + 3
        );
    }

    fn plain_files(diffs: &[FileDiff]) -> Vec<SidebarFile<'_>> {
        diffs
            .iter()
            .map(|file| SidebarFile {
                file,
                added: 0,
                deleted: 0,
                stage: None,
            })
            .collect()
    }

    fn texts(sidebar: &Sidebar) -> Vec<String> {
        sidebar.rows().iter().map(line_text).collect()
    }

    #[test]
    fn folding_a_directory_hides_its_subtree_until_unfolded() {
        let diffs = [fd("src/a.rs"), fd("src/b.rs"), fd("z.rs")];
        let files = plain_files(&diffs);
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.top(20);

        assert!(sidebar.toggle_selected_dir(20));
        assert_eq!(texts(&sidebar), ["▶ src/", "M z.rs"]);
        assert_eq!(sidebar.row_count(), 2);
        assert!(sidebar.selected_is_dir());

        assert!(sidebar.toggle_selected_dir(20));
        assert_eq!(sidebar.row_count(), 4);
        assert_eq!(line_text(&sidebar.rows()[0]), "▼ src/");
    }

    #[test]
    fn folded_directory_is_skipped_by_navigation_but_selects_its_whole_subtree() {
        let diffs = [fd("a.rs"), fd("src/b.rs"), fd("src/c.rs"), fd("z.rs")];
        let files = plain_files(&diffs);
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let order = sidebar.display_order();
        sidebar.move_down(20);
        sidebar.toggle_selected_dir(20);

        assert_eq!(sidebar.selection_display_range(), Some(1..3));
        assert_eq!(sidebar.display_order(), order);
        sidebar.move_down(20);
        assert_eq!(sidebar.selected_file_index(), Some(3));
        sidebar.bottom(20);
        assert_eq!(sidebar.selected(), 2);
        sidebar.move_up(20);
        assert_eq!(sidebar.selected_directory_path().as_deref(), Some("src/"));
    }

    #[test]
    fn nested_fold_survives_folding_and_unfolding_its_parent() {
        let diffs = [fd("src/a.rs"), fd("src/lib/b.rs"), fd("src/lib/c.rs")];
        let files = plain_files(&diffs);
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert!(sidebar.select_directory_path("src/lib/", 20));
        sidebar.toggle_selected_dir(20);
        sidebar.top(20);
        sidebar.toggle_selected_dir(20);
        assert_eq!(texts(&sidebar), ["▶ src/"]);

        sidebar.toggle_selected_dir(20);
        assert_eq!(texts(&sidebar), ["▼ src/", "  M a.rs", "  ▶ lib/"]);
    }

    #[test]
    fn selecting_a_hidden_file_selects_its_folded_directory() {
        let diffs = [fd("src/a.rs"), fd("src/b.rs"), fd("z.rs")];
        let files = plain_files(&diffs);
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.top(20);
        sidebar.toggle_selected_dir(20);
        sidebar.bottom(20);

        assert!(sidebar.select_file_index(1, 20));
        assert_eq!(sidebar.selected_directory_path().as_deref(), Some("src/"));
    }

    #[test]
    fn rebuilt_sidebar_keeps_folds_for_directories_that_still_exist() {
        let before = [fd("src/a.rs"), fd("tests/b.rs"), fd("z.rs")];
        let mut sidebar = Sidebar::build_with_icons(&plain_files(&before), &theme(), IconMode::Off);
        sidebar.top(20);
        sidebar.toggle_selected_dir(20);
        sidebar.move_down(20);
        sidebar.toggle_selected_dir(20);

        let after = [fd("src/a.rs"), fd("src/c.rs"), fd("z.rs")];
        let mut rebuilt = Sidebar::build_with_icons(&plain_files(&after), &theme(), IconMode::Off);
        rebuilt.apply_folds(sidebar.folds(), 20);

        assert_eq!(texts(&rebuilt), ["▶ src/", "M z.rs"]);
        assert!(rebuilt.selected_is_dir());
        assert_eq!(rebuilt.folds(), Folds(BTreeSet::from(["src/".to_string()])));
    }

    #[test]
    fn empty_files_produces_empty_sidebar() {
        let sidebar = Sidebar::build_with_icons(&[], &theme(), IconMode::Off);
        assert_eq!(sidebar.row_count(), 0);
        assert!(sidebar.rows().is_empty());
        assert_eq!(sidebar.selected_file_index(), None);
    }
}
