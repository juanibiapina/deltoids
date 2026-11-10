//! Files the reviewer marked as reviewed (`v`), and whether a view shows
//! them (`V`). Marks belong to the session: every view that shares a
//! [`ReviewMarks`] sees them, and a mark applies wherever the same change
//! to the same file shows up again. Any change to the file's diff leaves
//! the mark behind.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::sidebar::display_path;

use super::model::Model;

/// The session's reviewed changes: a display path and a hash of its diff.
#[derive(Clone, Default)]
pub(in crate::cli::browse) struct ReviewMarks(Rc<RefCell<HashSet<(String, u64)>>>);

#[derive(Default)]
pub(super) struct ReviewedFiles {
    marks: ReviewMarks,
    /// Reviewed files stay in the tree, marked.
    shown: bool,
}

impl ReviewedFiles {
    pub(super) fn new(marks: ReviewMarks) -> Self {
        Self {
            marks,
            shown: false,
        }
    }

    /// The marks, to share with another view.
    pub(super) fn marks(&self) -> ReviewMarks {
        self.marks.clone()
    }

    /// Mark every changed file in `indices`, or unmark them all when all
    /// are marked already. Visited files are never marked.
    pub(super) fn toggle(&mut self, model: &Model, indices: &[usize]) {
        let keys: Vec<(String, u64)> = indices
            .iter()
            .filter(|&&index| index < model.changed_files())
            .map(|&index| key(model, index))
            .collect();
        let mut marks = self.marks.0.borrow_mut();
        if keys.iter().all(|key| marks.contains(key)) {
            for key in &keys {
                marks.remove(key);
            }
        } else {
            marks.extend(keys);
        }
    }

    pub(super) fn shown(&self) -> bool {
        self.shown
    }

    pub(super) fn set_shown(&mut self, shown: bool) {
        self.shown = shown;
    }

    /// Per file index: reviewed.
    pub(super) fn marked(&self, model: &Model) -> Vec<bool> {
        let marks = self.marks.0.borrow();
        if marks.is_empty() {
            return vec![false; model.files.len()];
        }
        (0..model.files.len())
            .map(|index| index < model.changed_files() && marks.contains(&key(model, index)))
            .collect()
    }

    /// Whether any file of `model` is reviewed.
    pub(super) fn any(&self, model: &Model) -> bool {
        self.marked(model).contains(&true)
    }

    /// Per file index: hidden as reviewed. All false while shown.
    pub(super) fn hidden(&self, model: &Model) -> Vec<bool> {
        if self.shown {
            return Vec::new();
        }
        self.marked(model)
    }

    /// How many changed files are hidden as reviewed.
    pub(super) fn hidden_count(&self, model: &Model) -> usize {
        self.hidden(model)
            .into_iter()
            .filter(|&hidden| hidden)
            .count()
    }
}

fn key(model: &Model, index: usize) -> (String, u64) {
    (
        display_path(&model.files[index].file).to_string(),
        model.fingerprint(index),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::test_support::{model_from, resolved};

    fn model(files: &[(&str, &str)]) -> Model {
        let files: Vec<_> = files
            .iter()
            .map(|(path, after)| {
                let mut file = resolved(path);
                file.after = after.to_string();
                file
            })
            .collect();
        model_from(&files)
    }

    #[test]
    fn toggling_marks_then_unmarks() {
        let model = model(&[("a.rs", "a\n"), ("b.rs", "b\n")]);
        let mut reviewed = ReviewedFiles::default();
        reviewed.toggle(&model, &[0]);
        assert_eq!(reviewed.hidden(&model), [true, false]);
        assert_eq!(reviewed.hidden_count(&model), 1);
        reviewed.toggle(&model, &[0]);
        assert_eq!(reviewed.hidden_count(&model), 0);
    }

    #[test]
    fn a_partly_marked_selection_gets_marked_whole() {
        let model = model(&[("a.rs", "a\n"), ("b.rs", "b\n")]);
        let mut reviewed = ReviewedFiles::default();
        reviewed.toggle(&model, &[0]);
        reviewed.toggle(&model, &[0, 1]);
        assert_eq!(reviewed.marked(&model), [true, true]);
        reviewed.toggle(&model, &[0, 1]);
        assert_eq!(reviewed.marked(&model), [false, false]);
    }

    #[test]
    fn shown_files_stay_marked_but_not_hidden() {
        let model = model(&[("a.rs", "a\n")]);
        let mut reviewed = ReviewedFiles::default();
        reviewed.toggle(&model, &[0]);
        reviewed.set_shown(true);
        assert_eq!(reviewed.marked(&model), [true]);
        assert_eq!(reviewed.hidden_count(&model), 0);
    }

    #[test]
    fn a_mark_follows_the_same_change_into_another_view() {
        let before = model(&[("a.rs", "a\n"), ("b.rs", "b\n")]);
        let mut reviewed = ReviewedFiles::default();
        reviewed.toggle(&before, &[0, 1]);

        let other = ReviewedFiles::new(reviewed.marks());
        let after = model(&[("a.rs", "a\n"), ("b.rs", "b2\n"), ("c.rs", "c\n")]);
        assert_eq!(other.marked(&after), [true, false, false]);
    }
}
