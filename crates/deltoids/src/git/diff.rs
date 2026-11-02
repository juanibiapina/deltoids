//! Changes between two index-shaped file lists, with rename detection.
//!
//! Every diff the module produces goes through here: HEAD → index,
//! index → worktree, HEAD → worktree, and tree → tree for history. A tree
//! or the working tree is first expressed as a [`gix::index::State`], and
//! working-tree contents the object database lacks live in an
//! [`Overlay`].

use std::collections::HashMap;

use gix::ObjectId;
use gix::bstr::{BStr, BString};
use gix::diff::index::ChangeRef;
use gix::index::entry::Mode;

use super::Error;

/// Blob contents keyed by id, for working-tree files that are not in the
/// object database.
pub(super) type Overlay = HashMap<ObjectId, Vec<u8>>;

/// One side of a [`Change`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Side {
    pub path: BString,
    pub mode: Mode,
    pub id: ObjectId,
}

/// One file's change. `old: None` is an addition, `new: None` a deletion;
/// both present with different paths is a rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    pub old: Option<Side>,
    pub new: Option<Side>,
}

impl Change {
    /// The path the change is listed under: the new path, or the old one
    /// for a deletion.
    pub fn path(&self) -> &BStr {
        let side = self.new.as_ref().or(self.old.as_ref());
        side.map(|s| s.path.as_ref()).unwrap_or_default()
    }

    /// Whether the file changed kind (file, symlink, submodule).
    pub fn is_typechange(&self) -> bool {
        match (&self.old, &self.new) {
            (Some(old), Some(new)) => kind(old.mode) != kind(new.mode),
            _ => false,
        }
    }

    pub fn is_rename(&self) -> bool {
        match (&self.old, &self.new) {
            (Some(old), Some(new)) => old.path != new.path,
            _ => false,
        }
    }

    /// Old and new path, the missing side repeating the other, the way
    /// git status entries list them.
    pub fn paths(&self) -> [&BStr; 2] {
        let old = self.old.as_ref().or(self.new.as_ref());
        let new = self.new.as_ref().or(self.old.as_ref());
        [
            old.map(|s| s.path.as_ref()).unwrap_or_default(),
            new.map(|s| s.path.as_ref()).unwrap_or_default(),
        ]
    }
}

/// The kind of entry a mode names: regular files of either permission
/// are one kind.
pub(super) fn kind(mode: Mode) -> Mode {
    if mode == Mode::FILE_EXECUTABLE {
        Mode::FILE
    } else {
        mode
    }
}

/// Changes that turn `lhs` into `rhs`, sorted by [`Change::path`].
/// Deletions and additions with at least 50% similar content pair up as
/// renames, whatever `diff.renames` says.
pub(super) fn changes(
    repo: &gix::Repository,
    lhs: &gix::index::State,
    rhs: &gix::index::State,
    overlay: &Overlay,
) -> Result<Vec<Change>, Error> {
    let objects = Objects { repo, overlay };
    let mut resource_cache = repo.diff_resource_cache_for_tree_diff()?;
    let mut pathspec = gix::pathspec::Search::from_specs(None, None, std::path::Path::new(""))?;
    let mut out = Vec::new();
    gix::diff::index(
        lhs,
        rhs,
        |change| {
            out.push(convert(change));
            Ok(gix::diff::index::Action::Continue(()))
        },
        Some(gix::diff::index::RewriteOptions {
            resource_cache: &mut resource_cache,
            find: &objects,
            rewrites: gix::diff::Rewrites::default(),
        }),
        &mut pathspec,
        &mut |_, _, _, _| false,
    )?;
    out.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(out)
}

/// Changes from tree `old` (`None` for an empty tree) to tree `new`,
/// sorted by [`Change::path`], with the same rename rule as
/// [`changes`]. Subtrees with equal ids are skipped unread.
pub(super) fn tree_changes(
    repo: &gix::Repository,
    old: Option<ObjectId>,
    new: ObjectId,
) -> Result<Vec<Change>, Error> {
    use gix::object::tree::diff::ChangeDetached;
    let old = old.map(|id| repo.find_tree(id)).transpose()?;
    let new = repo.find_tree(new)?;
    let options = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
    let side = |path: BString, mode: gix::objs::tree::EntryMode, id: ObjectId| {
        Some(Side {
            path,
            mode: entry_mode(mode)?,
            id,
        })
    };
    let mut out = Vec::new();
    for change in repo.diff_tree_to_tree(old.as_ref(), &new, options)? {
        let change = match change {
            ChangeDetached::Addition {
                location,
                entry_mode,
                id,
                ..
            } => Change {
                old: None,
                new: side(location, entry_mode, id),
            },
            ChangeDetached::Deletion {
                location,
                entry_mode,
                id,
                ..
            } => Change {
                old: side(location, entry_mode, id),
                new: None,
            },
            ChangeDetached::Modification {
                location,
                previous_entry_mode,
                previous_id,
                entry_mode,
                id,
            } => Change {
                old: side(location.clone(), previous_entry_mode, previous_id),
                new: side(location, entry_mode, id),
            },
            ChangeDetached::Rewrite {
                source_location,
                source_entry_mode,
                source_id,
                location,
                entry_mode,
                id,
                ..
            } => Change {
                old: side(source_location, source_entry_mode, source_id),
                new: side(location, entry_mode, id),
            },
        };
        if change.old.is_some() || change.new.is_some() {
            out.push(change);
        }
    }
    out.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(out)
}

/// The index mode of a tree entry, `None` for a subtree.
fn entry_mode(mode: gix::objs::tree::EntryMode) -> Option<Mode> {
    use gix::objs::tree::EntryKind;
    match mode.kind() {
        EntryKind::Tree => None,
        EntryKind::Blob => Some(Mode::FILE),
        EntryKind::BlobExecutable => Some(Mode::FILE_EXECUTABLE),
        EntryKind::Link => Some(Mode::SYMLINK),
        EntryKind::Commit => Some(Mode::COMMIT),
    }
}

fn convert(change: ChangeRef<'_, '_>) -> Change {
    let side = |path: &BStr, mode: Mode, id: &gix::hash::oid| Side {
        path: path.to_owned(),
        mode,
        id: id.to_owned(),
    };
    match change {
        ChangeRef::Addition {
            location,
            entry_mode,
            id,
            ..
        } => Change {
            old: None,
            new: Some(side(&location, entry_mode, &id)),
        },
        ChangeRef::Deletion {
            location,
            entry_mode,
            id,
            ..
        } => Change {
            old: Some(side(&location, entry_mode, &id)),
            new: None,
        },
        ChangeRef::Modification {
            location,
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
            ..
        } => Change {
            old: Some(side(&location, previous_entry_mode, &previous_id)),
            new: Some(side(&location, entry_mode, &id)),
        },
        ChangeRef::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            location,
            entry_mode,
            id,
            ..
        } => Change {
            old: Some(side(&source_location, source_entry_mode, &source_id)),
            new: Some(side(&location, entry_mode, &id)),
        },
    }
}

/// The object database with the [`Overlay`] on top.
struct Objects<'a> {
    repo: &'a gix::Repository,
    overlay: &'a Overlay,
}

impl gix::objs::Find for Objects<'_> {
    fn try_find<'a>(
        &self,
        id: &gix::hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix::error::Result<Option<gix::objs::Data<'a>>> {
        if let Some(data) = self.overlay.get(id) {
            buffer.clear();
            buffer.extend_from_slice(data);
            return Ok(Some(gix::objs::Data::new(
                buffer,
                gix::objs::Kind::Blob,
                gix::hash::Kind::Sha1,
            )));
        }
        self.repo.objects.try_find(id, buffer)
    }
}

impl gix::objs::FindHeader for Objects<'_> {
    fn try_header(&self, id: &gix::hash::oid) -> gix::error::Result<Option<gix::objs::Header>> {
        if let Some(data) = self.overlay.get(id) {
            return Ok(Some(gix::objs::Header {
                kind: gix::objs::Kind::Blob,
                size: data.len() as u64,
            }));
        }
        self.repo.objects.try_header(id)
    }
}

/// The bytes of `side`: a submodule's commit line, an overlay entry, or a
/// blob from the object database.
pub(super) fn content(
    repo: &gix::Repository,
    overlay: &Overlay,
    side: &Side,
) -> Result<Vec<u8>, Error> {
    if side.mode == Mode::COMMIT {
        return Ok(format!("Subproject commit {}\n", side.id).into_bytes());
    }
    if let Some(data) = overlay.get(&side.id) {
        return Ok(data.clone());
    }
    Ok(repo.find_blob(side.id)?.take_data())
}
