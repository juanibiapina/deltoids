use std::cmp::Ordering;

use git2::{Delta, Diff, DiffDelta, DiffFindOptions, DiffOptions, Error, ErrorCode, Repository};

use super::{FileStageStatus, StageChange, WorkingTreeSnapshot};

pub(super) fn read(repo: &Repository) -> Result<WorkingTreeSnapshot, Error> {
    let head = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::UnbornBranch) => None,
        Err(error) => return Err(error),
    };
    let mut index = repo.index()?;
    index.read(false)?;
    let mut options = super::working_tree_diff_options();
    let mut patch = repo.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut options))?;
    let mut workdir = repo.diff_index_to_workdir(Some(&index), Some(&mut options))?;
    // Merge copies the source deltas. Status needs the original index paths.
    patch.merge(&workdir)?;
    let patch = super::print_working_tree_patch(patch)?;

    let mut status_options = DiffOptions::new();
    status_options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_typechange(true);
    let mut staged =
        repo.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut status_options))?;
    // Matches libgit2 status: include untracked files as rename targets.
    let mut find = DiffFindOptions::new();
    find.for_untracked(true);
    staged.find_similar(Some(&mut find))?;
    workdir.find_similar(Some(&mut find))?;
    Ok(WorkingTreeSnapshot {
        patch,
        stages: pair(&staged, &workdir),
    })
}

fn pair(staged: &Diff<'_>, workdir: &Diff<'_>) -> Vec<FileStageStatus> {
    let ignore_case = staged.is_sorted_icase() && workdir.is_sorted_icase();
    let mut left: Vec<_> = staged.deltas().collect();
    let mut right: Vec<_> = workdir.deltas().collect();
    left.sort_by(|a, b| compare(index_path(a, true), index_path(b, true), ignore_case));
    right.sort_by(|a, b| compare(index_path(a, false), index_path(b, false), ignore_case));
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    let mut out = Vec::new();
    while left.peek().is_some() || right.peek().is_some() {
        let order = match (left.peek(), right.peek()) {
            (Some(x), Some(y)) => compare(index_path(x, true), index_path(y, false), ignore_case),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        let (x, y) = match order {
            Ordering::Less => (left.next(), None),
            Ordering::Greater => (None, right.next()),
            Ordering::Equal => (left.next(), right.next()),
        };
        let staged = x.as_ref().and_then(|d| staged_change(d.status()));
        let unstaged = y.as_ref().and_then(|d| workdir_change(d.status()));
        if staged.is_none() && unstaged.is_none() {
            continue;
        }
        // The existing status interface prefers the staged delta's new path.
        let path = x.as_ref().or(y.as_ref()).and_then(|d| d.new_file().path());
        if let Some(path) = path {
            out.push(FileStageStatus {
                path: path.to_string_lossy().into_owned(),
                staged,
                unstaged,
            });
        }
    }
    out
}

fn index_path<'a>(delta: &DiffDelta<'a>, staged: bool) -> &'a [u8] {
    let file = if staged {
        delta.new_file()
    } else {
        delta.old_file()
    };
    file.path_bytes().unwrap_or_default()
}

fn compare(a: &[u8], b: &[u8], ignore_case: bool) -> Ordering {
    if ignore_case {
        a.iter()
            .map(u8::to_ascii_lowercase)
            .cmp(b.iter().map(u8::to_ascii_lowercase))
    } else {
        a.cmp(b)
    }
}

fn staged_change(delta: Delta) -> Option<StageChange> {
    match delta {
        Delta::Added | Delta::Copied => Some(StageChange::Added),
        Delta::Modified => Some(StageChange::Modified),
        Delta::Deleted => Some(StageChange::Deleted),
        Delta::Renamed => Some(StageChange::Renamed),
        Delta::Typechange => Some(StageChange::TypeChanged),
        _ => None,
    }
}

fn workdir_change(delta: Delta) -> Option<StageChange> {
    match delta {
        Delta::Added | Delta::Copied | Delta::Untracked => Some(StageChange::Untracked),
        Delta::Modified => Some(StageChange::Modified),
        Delta::Deleted => Some(StageChange::Deleted),
        Delta::Renamed => Some(StageChange::Renamed),
        Delta::Typechange => Some(StageChange::TypeChanged),
        _ => None,
    }
}
