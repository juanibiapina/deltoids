//! One working-tree scan: HEAD, the index, and the files on disk, turned
//! into the net patch, the split patches, and both staging columns.
//!
//! The working tree is expressed as an index: the real index with each
//! changed file's id and mode replaced by what is on disk, deleted files
//! removed, and untracked and conflicted files added. Its blobs go in an
//! [`Overlay`]. Each diff is then index against index (see
//! [`super::diff`]).

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::index::entry::{Mode, Stage};
use gix::status::index_worktree::Item;
use gix::status::plumbing::index_as_worktree::{Change as WorktreeChange, EntryStatus};

use super::diff::{self, Change, Overlay};
use super::{Error, FileStageStatus, StageChange, WorkingTreeSnapshot, patch, path_str};

/// Read the snapshot. An unborn `HEAD` reads as an empty tree; a `HEAD`
/// that fails to resolve otherwise is an error.
/// Also returns the working-tree contents the scan read.
pub(super) fn read(repo: &gix::Repository) -> Result<(WorkingTreeSnapshot, Overlay), Error> {
    let scan = Scan::read(repo)?;
    let net = diff::changes(repo, &scan.head, &scan.worktree, &scan.overlay)?;
    let (staged, unstaged) = scan.columns(repo)?;
    let (stages, dual) = pair(&staged, &unstaged, ignore_case(repo));
    let print =
        |changes: &[Change]| patch::write(changes, |side| diff::content(repo, &scan.overlay, side));
    let in_dual = |change: &&Change| change.paths().iter().any(|p| dual.contains(*p));
    let staged_dual: Vec<Change> = staged.iter().filter(in_dual).cloned().collect();
    let unstaged_dual: Vec<Change> = unstaged.iter().filter(in_dual).cloned().collect();
    let snapshot = WorkingTreeSnapshot {
        patch: print(&net)?,
        staged_patch: print(&staged_dual)?,
        unstaged_patch: print(&unstaged_dual)?,
        stages,
    };
    Ok((snapshot, scan.overlay))
}

/// Only the staging columns of [`read`], without patch text.
pub(super) fn stages(repo: &gix::Repository) -> Result<Vec<FileStageStatus>, Error> {
    let scan = Scan::read(repo)?;
    let (staged, unstaged) = scan.columns(repo)?;
    Ok(pair(&staged, &unstaged, ignore_case(repo)).0)
}

fn ignore_case(repo: &gix::Repository) -> bool {
    repo.config_snapshot()
        .boolean("core.ignoreCase")
        .unwrap_or(false)
}

fn without(changes: Vec<Change>, paths: &BTreeSet<BString>) -> Vec<Change> {
    changes
        .into_iter()
        .filter(|change| !change.paths().iter().any(|p| paths.contains(*p)))
        .collect()
}

struct Scan {
    /// `HEAD`'s tree as an index, empty when unborn.
    head: gix::index::State,
    /// The index without conflict stages.
    index: gix::index::State,
    /// What is on disk, as an index.
    worktree: gix::index::State,
    /// Paths with merge conflicts. Their files are part of `worktree`.
    conflicted: BTreeSet<BString>,
    overlay: Overlay,
}

/// What the scan found on disk for one path.
enum OnDisk {
    Missing,
    Blob(Mode, ObjectId),
    Submodule(ObjectId),
}

impl Scan {
    /// The staged (HEAD → index) and unstaged (index → worktree)
    /// changes, conflicted files left out.
    fn columns(&self, repo: &gix::Repository) -> Result<(Vec<Change>, Vec<Change>), Error> {
        let staged = diff::changes(repo, &self.head, &self.index, &self.overlay)?;
        let unstaged = diff::changes(repo, &self.index, &self.worktree, &self.overlay)?;
        Ok((
            without(staged, &self.conflicted),
            without(unstaged, &self.conflicted),
        ))
    }

    fn read(repo: &gix::Repository) -> Result<Self, Error> {
        let head = match head_tree(repo)? {
            Some(tree) => repo.index_from_tree(&tree)?.into(),
            None => gix::index::State::new(repo.object_hash()),
        };
        let persisted = repo.index_or_empty()?;
        let mut reader = Reader {
            workdir: repo
                .workdir()
                .ok_or("the repository has no working tree")?
                .to_owned(),
            filemode: repo
                .config_snapshot()
                .boolean("core.fileMode")
                .unwrap_or(true),
            overlay: Overlay::new(),
        };
        let walk = Walk::read(repo, &persisted, &mut reader)?;
        let mut index: gix::index::State = gix::index::File::clone(&persisted).into();
        index.remove_entries(|_, path, entry| {
            entry.stage() != Stage::Unconflicted || walk.conflicted.contains(path)
        });
        let worktree = walk.worktree(&index, &mut reader)?;
        Ok(Scan {
            head,
            index,
            worktree,
            conflicted: walk.conflicted,
            overlay: reader.overlay,
        })
    }
}

/// Reads working-tree files into the [`Overlay`].
struct Reader {
    workdir: std::path::PathBuf,
    filemode: bool,
    overlay: Overlay,
}

impl Reader {
    /// Read the file at `path`, whose index mode is `old` if tracked.
    fn read(&mut self, path: &BStr, old: Option<Mode>) -> Result<OnDisk, Error> {
        Ok(match read_file(&self.workdir, path, self.filemode, old)? {
            Some((mode, data)) => {
                let id = super::blob_id(&data)?;
                self.overlay.insert(id, data);
                OnDisk::Blob(mode, id)
            }
            None => OnDisk::Missing,
        })
    }
}

/// What the status walk found: changed tracked files, untracked files,
/// and conflicted paths.
#[derive(Default)]
struct Walk {
    found: HashMap<BString, OnDisk>,
    untracked: Vec<BString>,
    conflicted: BTreeSet<BString>,
}

impl Walk {
    fn read(
        repo: &gix::Repository,
        persisted: &gix::worktree::Index,
        reader: &mut Reader,
    ) -> Result<Self, Error> {
        let status = repo
            .status(gix::progress::Discard)?
            .index(persisted.clone().into())
            .untracked_files(gix::status::UntrackedFiles::Files)
            .index_worktree_rewrites(None)
            .index_worktree_submodules(gix::status::Submodule::Given {
                ignore: gix::submodule::config::Ignore::Dirty,
                check_dirty: false,
            })
            .dirwalk_options(|options| options.empty_patterns_match_prefix(false))
            .into_index_worktree_iter(Vec::new())?;
        let mut walk = Walk::default();
        for item in status {
            walk.record(item?, reader)?;
        }
        Ok(walk)
    }

    fn record(&mut self, item: Item, reader: &mut Reader) -> Result<(), Error> {
        match item {
            Item::Modification {
                rela_path,
                entry,
                status,
                ..
            } => self.record_tracked(rela_path, &entry, status, reader)?,
            Item::DirectoryContents { entry, .. } => {
                let file = matches!(
                    entry.disk_kind,
                    Some(gix::dir::entry::Kind::File | gix::dir::entry::Kind::Symlink)
                );
                if entry.status == gix::dir::entry::Status::Untracked && file {
                    self.untracked.push(entry.rela_path);
                }
            }
            Item::Rewrite { .. } => {}
        }
        Ok(())
    }

    fn record_tracked(
        &mut self,
        path: BString,
        entry: &gix::index::Entry,
        status: EntryStatus<(), gix::submodule::Status>,
        reader: &mut Reader,
    ) -> Result<(), Error> {
        let disk = match status {
            EntryStatus::Conflict { .. } => {
                self.conflicted.insert(path);
                return Ok(());
            }
            EntryStatus::Change(WorktreeChange::Removed) => OnDisk::Missing,
            EntryStatus::Change(
                WorktreeChange::Type { .. } | WorktreeChange::Modification { .. },
            ) => reader.read(path.as_ref(), Some(entry.mode))?,
            EntryStatus::Change(WorktreeChange::SubmoduleModification(submodule)) => {
                match submodule.checked_out_head_id {
                    Some(head) if head != entry.id => OnDisk::Submodule(head),
                    _ => return Ok(()),
                }
            }
            EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => return Ok(()),
        };
        self.found.insert(path, disk);
        Ok(())
    }

    /// `index` with what is on disk: changed entries updated, deleted
    /// ones removed, untracked and conflicted files added.
    fn worktree(
        &self,
        index: &gix::index::State,
        reader: &mut Reader,
    ) -> Result<gix::index::State, Error> {
        let mut worktree = index.clone();
        let mut missing = BTreeSet::new();
        for (path, disk) in &self.found {
            let Ok(position) = worktree.entry_index_by_path(path.as_ref()) else {
                continue;
            };
            let entry = &mut worktree.entries_mut()[position];
            match disk {
                OnDisk::Missing => {
                    missing.insert(path.clone());
                }
                OnDisk::Blob(mode, id) => {
                    entry.mode = *mode;
                    entry.id = *id;
                }
                OnDisk::Submodule(id) => entry.id = *id,
            }
        }
        worktree.remove_entries(|_, path, _| missing.contains(path));
        for path in self.untracked.iter().chain(self.conflicted.iter()) {
            if let OnDisk::Blob(mode, id) = reader.read(path.as_ref(), None)? {
                worktree.dangerously_push_entry(
                    Default::default(),
                    id,
                    gix::index::entry::Flags::empty(),
                    mode,
                    path.as_ref(),
                );
            }
        }
        worktree.sort_entries();
        Ok(worktree)
    }
}

fn head_tree(repo: &gix::Repository) -> Result<Option<ObjectId>, Error> {
    if repo.head()?.is_unborn() {
        return Ok(None);
    }
    Ok(Some(repo.head_tree_id()?.detach()))
}

/// The mode and bytes of the file at `path`: a symlink's target, or a
/// file's content. `None` when nothing trackable is there. Without
/// `core.fileMode`, a file keeps the executable bit of its `old` mode.
fn read_file(
    workdir: &Path,
    path: &BStr,
    filemode: bool,
    old: Option<Mode>,
) -> Result<Option<(Mode, Vec<u8>)>, Error> {
    let full = workdir.join(gix::path::from_bstr(path)?);
    let metadata = match std::fs::symlink_metadata(&full) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(&full)?;
        let bytes = gix::path::into_bstr(target)?.into_owned();
        return Ok(Some((Mode::SYMLINK, bytes.into())));
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    let executable = if filemode {
        is_executable(&metadata)
    } else {
        old == Some(Mode::FILE_EXECUTABLE)
    };
    let data = match std::fs::read(&full) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mode = if executable {
        Mode::FILE_EXECUTABLE
    } else {
        Mode::FILE
    };
    Ok(Some((mode, data)))
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}

/// Pair both columns per file by their index path: the staged change's
/// new path against the unstaged change's old path. Also returns the
/// paths of files with both columns, rename companions included, for the
/// split patches.
fn pair(
    staged: &[Change],
    unstaged: &[Change],
    ignore_case: bool,
) -> (Vec<FileStageStatus>, BTreeSet<BString>) {
    let mut left: Vec<&Change> = staged.iter().collect();
    let mut right: Vec<&Change> = unstaged.iter().collect();
    left.sort_by(|a, b| compare(a.paths()[1], b.paths()[1], ignore_case));
    right.sort_by(|a, b| compare(a.paths()[0], b.paths()[0], ignore_case));
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    let mut out = Vec::new();
    let mut dual = BTreeSet::new();
    loop {
        let order = match (left.peek(), right.peek()) {
            (Some(x), Some(y)) => compare(x.paths()[1], y.paths()[0], ignore_case),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        let (x, y) = match order {
            Ordering::Less => (left.next(), None),
            Ordering::Greater => (None, right.next()),
            Ordering::Equal => (left.next(), right.next()),
        };
        let staged = x.map(|c| column(c, false));
        let unstaged = y.map(|c| column(c, true));
        let paths: Vec<&BStr> = x.iter().chain(y.iter()).flat_map(|c| c.paths()).collect();
        if staged.is_some() && unstaged.is_some() {
            dual.extend(paths.iter().map(|p| BString::from(p.as_bytes())));
        }
        let path = x.or(y).map(|c| c.paths()[1]).unwrap_or_default();
        out.push(FileStageStatus {
            path: path_str(path),
            paths: paths.iter().map(|p| path_str(p)).collect(),
            staged,
            unstaged,
        });
    }
    (out, dual)
}

fn compare(a: &BStr, b: &BStr, ignore_case: bool) -> Ordering {
    if ignore_case {
        a.iter()
            .map(u8::to_ascii_lowercase)
            .cmp(b.iter().map(u8::to_ascii_lowercase))
    } else {
        a.cmp(b)
    }
}

/// A change's status letter. Additions in the worktree column are
/// untracked files.
fn column(change: &Change, worktree: bool) -> StageChange {
    match (&change.old, &change.new) {
        (None, _) if worktree => StageChange::Untracked,
        (None, _) => StageChange::Added,
        (_, None) => StageChange::Deleted,
        _ if change.is_rename() => StageChange::Renamed,
        _ if change.is_typechange() => StageChange::TypeChanged,
        _ => StageChange::Modified,
    }
}
