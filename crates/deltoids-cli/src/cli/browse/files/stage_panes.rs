//! Which staging column the diff pane shows, with which files.
//!
//! The pane shows one column at a time. A selection (one file or a
//! directory subtree) with both staged and unstaged changes shows the
//! column the reviewer picked, and `s` switches to the other. A selection
//! with one kind of change always shows that kind. The pane lists the
//! selected files that belong to its column, in sidebar order.

use std::ops::Range;

use super::model::{Column, Model};

/// Per-column file orders for one model and sidebar order. Rebuilt when
/// the model or its staging columns change; [`StagePanes::view`] is then
/// constant time per frame.
pub(super) struct StagePanes {
    has_stages: bool,
    staged: ColumnOrder,
    unstaged: ColumnOrder,
}

/// The files of one column in display order, plus how many of them come
/// before each display position.
struct ColumnOrder {
    files: Vec<usize>,
    before: Vec<usize>,
}

impl ColumnOrder {
    fn new(display_order: &[usize], member: impl Fn(usize) -> bool) -> Self {
        let mut files = Vec::new();
        let mut before = Vec::with_capacity(display_order.len() + 1);
        before.push(0);
        for &index in display_order {
            if member(index) {
                files.push(index);
            }
            before.push(files.len());
        }
        Self { files, before }
    }

    /// The slice of `files` a display-order selection covers.
    fn slice(&self, selection: &Range<usize>) -> Range<usize> {
        let last = self.before.len() - 1;
        self.before[selection.start.min(last)]..self.before[selection.end.min(last)]
    }
}

/// What the pane's title names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneTitle {
    /// No staging information (no repo or empty): a plain diff.
    Diff,
    /// The selection only has changes in this column.
    One(Column),
    /// The selection has both columns; the pane shows `column`'s.
    Both(Column),
}

/// The pane for one selection: the column it shows, its title, every file
/// of that column in display order, and the selection's slice of them.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PaneSpec<'a> {
    pub(super) column: Column,
    pub(super) title: PaneTitle,
    pub(super) order: &'a [usize],
    pub(super) range: Option<Range<usize>>,
}

impl StagePanes {
    /// Sort `model`'s files into columns. A file belongs to the staged
    /// column when it has staged changes, and to the unstaged column when
    /// it has unstaged changes or no staging record at all (no repo).
    pub(super) fn new(model: &Model, display_order: &[usize]) -> Self {
        let staged = ColumnOrder::new(display_order, |index| {
            model.stage(index).is_some_and(|stage| stage.is_staged())
        });
        let unstaged = ColumnOrder::new(display_order, |index| {
            model.stage(index).is_none_or(|stage| stage.is_unstaged())
        });
        Self {
            has_stages: !model.stages.is_empty(),
            staged,
            unstaged,
        }
    }

    /// The pane for a display-order `selection`: `preferred`'s column when
    /// the selection has both staged and unstaged files, otherwise the one
    /// column it has (unstaged when it has none).
    pub(super) fn view(&self, selection: Option<Range<usize>>, preferred: Column) -> PaneSpec<'_> {
        let Some(selection) = selection else {
            return self.pane(Column::Unstaged, None, false);
        };
        let staged = self.staged.slice(&selection);
        let unstaged = self.unstaged.slice(&selection);
        match (staged.is_empty(), unstaged.is_empty()) {
            (false, false) => match preferred {
                Column::Staged => self.pane(Column::Staged, Some(staged), true),
                Column::Unstaged => self.pane(Column::Unstaged, Some(unstaged), true),
            },
            (false, true) => self.pane(Column::Staged, Some(staged), false),
            _ => self.pane(Column::Unstaged, Some(unstaged), false),
        }
    }

    fn pane(&self, column: Column, range: Option<Range<usize>>, both: bool) -> PaneSpec<'_> {
        let order = match column {
            Column::Staged => &self.staged,
            Column::Unstaged => &self.unstaged,
        };
        let title = match (self.has_stages, both) {
            (false, _) => PaneTitle::Diff,
            (true, false) => PaneTitle::One(column),
            (true, true) => PaneTitle::Both(column),
        };
        PaneSpec {
            column,
            title,
            order: &order.files,
            range,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::test_support::model_of;
    use crate::sidebar::{ChangeKind, StageStatus};

    const STAGED: StageStatus = StageStatus {
        staged: Some(ChangeKind::Modified),
        unstaged: None,
    };
    const UNSTAGED: StageStatus = StageStatus {
        staged: None,
        unstaged: Some(ChangeKind::Modified),
    };
    const BOTH: StageStatus = StageStatus {
        staged: Some(ChangeKind::Modified),
        unstaged: Some(ChangeKind::Modified),
    };
    const UNTRACKED: StageStatus = StageStatus {
        staged: None,
        unstaged: Some(ChangeKind::Untracked),
    };

    fn model(files: &[(&str, StageStatus)]) -> Model {
        let paths: Vec<&str> = files.iter().map(|(path, _)| *path).collect();
        let mut model = model_of(&paths);
        model.stages = files
            .iter()
            .map(|(path, stage)| (path.to_string(), *stage))
            .collect();
        model
    }

    fn files(pane: &PaneSpec<'_>) -> Vec<usize> {
        pane.order[pane.range.clone().unwrap()].to_vec()
    }

    #[test]
    fn a_file_with_both_columns_shows_the_preferred_one() {
        let model = model(&[("a.rs", BOTH)]);
        let panes = StagePanes::new(&model, &[0]);
        for column in [Column::Staged, Column::Unstaged] {
            let pane = panes.view(Some(0..1), column);
            assert_eq!(pane.column, column);
            assert_eq!(pane.title, PaneTitle::Both(column));
            assert_eq!(files(&pane), [0]);
        }
    }

    #[test]
    fn a_single_column_file_shows_its_column_whatever_is_preferred() {
        let model = model(&[("a.rs", STAGED), ("b.rs", UNSTAGED), ("c.rs", UNTRACKED)]);
        let panes = StagePanes::new(&model, &[0, 1, 2]);
        for preferred in [Column::Staged, Column::Unstaged] {
            let staged = panes.view(Some(0..1), preferred);
            assert_eq!(staged.title, PaneTitle::One(Column::Staged));
            let unstaged = panes.view(Some(1..2), preferred);
            assert_eq!(unstaged.title, PaneTitle::One(Column::Unstaged));
            let untracked = panes.view(Some(2..3), preferred);
            assert_eq!(untracked.title, PaneTitle::One(Column::Unstaged));
            assert_eq!(files(&untracked), [2]);
        }
    }

    #[test]
    fn a_directory_with_both_kinds_lists_only_the_shown_columns_files() {
        let model = model(&[
            ("x.rs", UNSTAGED),
            ("dir/a.rs", STAGED),
            ("dir/b.rs", UNSTAGED),
            ("y.rs", STAGED),
        ]);
        let panes = StagePanes::new(&model, &[0, 1, 2, 3]);
        let staged = panes.view(Some(1..3), Column::Staged);
        assert_eq!(staged.title, PaneTitle::Both(Column::Staged));
        assert_eq!(files(&staged), [1]);
        assert_eq!(staged.order, [1, 3]);
        let unstaged = panes.view(Some(1..3), Column::Unstaged);
        assert_eq!(files(&unstaged), [2]);
        assert_eq!(unstaged.order, [0, 2]);
    }

    #[test]
    fn display_order_decides_file_order() {
        let model = model(&[("a.rs", BOTH), ("b.rs", BOTH), ("c.rs", UNSTAGED)]);
        let panes = StagePanes::new(&model, &[2, 1, 0]);
        assert_eq!(files(&panes.view(Some(0..3), Column::Staged)), [1, 0]);
        assert_eq!(files(&panes.view(Some(0..3), Column::Unstaged)), [2, 1, 0]);
        assert_eq!(files(&panes.view(Some(2..3), Column::Staged)), [0]);
    }

    #[test]
    fn without_staging_records_the_pane_is_a_plain_diff_of_every_file() {
        let model = model_of(&["a.rs", "b.rs"]);
        let panes = StagePanes::new(&model, &[0, 1]);
        let pane = panes.view(Some(0..2), Column::Staged);
        assert_eq!(pane.title, PaneTitle::Diff);
        assert_eq!(pane.column, Column::Unstaged);
        assert_eq!(files(&pane), [0, 1]);
    }

    #[test]
    fn an_empty_model_shows_an_empty_plain_pane() {
        let model = Model::empty();
        let panes = StagePanes::new(&model, &[]);
        let pane = panes.view(None, Column::Staged);
        assert_eq!(pane.title, PaneTitle::Diff);
        assert!(pane.order.is_empty());
    }
}
