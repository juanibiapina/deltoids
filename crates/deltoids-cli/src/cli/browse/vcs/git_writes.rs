//! The git adapter's writes: stage, unstage, and discard through the
//! index and the git CLI.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use deltoids::git::Repo;
use gix::ObjectId;

use super::DiscardKind;

const SUBMODULE: u32 = 0o160000;

#[derive(Debug, PartialEq, Eq)]
struct Change {
    display_path: PathBuf,
    paths: BTreeSet<PathBuf>,
    staged: bool,
    unstaged: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    path: PathBuf,
    index: Option<(ObjectId, u32)>,
    worktree: Option<(u32, Option<ObjectId>)>,
}

/// The changes of one selection, as read before a write. Comparing two
/// reads tells whether the selection changed in between.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct State {
    head: Option<ObjectId>,
    changes: Vec<Change>,
    files: Vec<FileState>,
}

pub(super) fn toggle_stage(workdir: &Path, targets: Vec<PathBuf>) -> Result<String, String> {
    let repo = open(workdir)?;
    let state = read(&repo, &targets, false)?;
    if state.changes.is_empty() {
        return Ok(String::new());
    }
    if state.changes.iter().any(|c| c.unstaged) {
        stage_changes(&repo, &state)?;
        Ok("Staged selection".into())
    } else {
        let paths = paths(&state);
        reset_index(&repo, &paths)?;
        Ok("Unstaged selection".into())
    }
}

pub(super) fn toggle_stage_all(workdir: &Path) -> Result<String, String> {
    let repo = open(workdir)?;
    let state = read_scope(&repo, None, false)?;
    if state.changes.is_empty() {
        return Ok("No changes".into());
    }
    if state.changes.iter().any(|c| c.unstaged) {
        stage_changes(&repo, &state)?;
        Ok("Staged all files".into())
    } else {
        reset_index(&repo, &paths(&state))?;
        Ok("Unstaged all files".into())
    }
}

fn stage_changes(repo: &Repository, state: &State) -> Result<(), String> {
    let index = repo.index()?;
    let paths: BTreeSet<_> = state
        .changes
        .iter()
        .filter(|c| c.unstaged)
        .flat_map(|c| c.paths.iter())
        .filter(|p| {
            index_entry(&index, p).is_some() || repo.workdir.join(p).symlink_metadata().is_ok()
        })
        .cloned()
        .collect();
    git(&repo.workdir, &["add", "-A"], &paths)
}

pub(super) fn read_selection(workdir: &Path, targets: &[PathBuf]) -> Result<State, String> {
    read(&open(workdir)?, targets, true)
}

impl State {
    pub(super) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(super) fn has_unstaged(&self) -> bool {
        self.changes.iter().any(|c| c.unstaged)
    }

    /// The menu's subject: the one file's path, or how many files.
    pub(super) fn label(&self) -> String {
        if self.changes.len() == 1 {
            self.changes[0].display_path.display().to_string()
        } else {
            format!("{} selected files", self.changes.len())
        }
    }

    /// Every path each change touches, rename sources included.
    pub(super) fn change_paths(&self) -> Vec<Vec<PathBuf>> {
        self.changes
            .iter()
            .map(|c| c.paths.iter().cloned().collect())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn retained_hashes(&self) -> usize {
        self.files
            .iter()
            .filter_map(|f| f.worktree.as_ref())
            .filter_map(|(_, hash)| hash.as_ref())
            .map(std::mem::size_of_val)
            .sum()
    }
}

/// Discard `state`'s changes the way `kind` says.
pub(super) fn discard(workdir: &Path, state: &State, kind: DiscardKind) -> Result<(), String> {
    apply(&open(workdir)?, state, kind)
}

/// The repository a write acts on: `gix` for index and tree lookups,
/// and the library's [`Repo`] for the staging columns.
struct Repository {
    git: gix::Repository,
    status: Repo,
    workdir: PathBuf,
}

impl Repository {
    fn index(&self) -> Result<gix::worktree::Index, String> {
        self.git.index_or_empty().map_err(message)
    }

    /// `HEAD`'s tree, or `None` before the first commit.
    fn head(&self) -> Result<Option<gix::Tree<'_>>, String> {
        if self.git.head().map_err(message)?.is_unborn() {
            return Ok(None);
        }
        self.git.head_tree().map(Some).map_err(message)
    }
}

fn open(workdir: &Path) -> Result<Repository, String> {
    let git = gix::open(workdir).map_err(message)?;
    let workdir = git
        .workdir()
        .ok_or("Git actions require a working tree")?
        .to_owned();
    let status = Repo::discover_at(&workdir).ok_or("Not a git repository")?;
    Ok(Repository {
        git,
        status,
        workdir,
    })
}

/// The stage-0 index entry at `path`: its id and mode bits.
fn index_entry(index: &gix::index::State, path: &Path) -> Option<(ObjectId, u32)> {
    let path = gix::path::into_bstr(path).ok()?;
    let entry = index.entry_by_path(path.as_ref())?;
    Some((entry.id, entry.mode.bits()))
}

/// The entry at `path` in `tree`: its id and mode bits.
fn tree_entry(tree: &gix::Tree<'_>, path: &Path) -> Option<(ObjectId, u32)> {
    let entry = tree.lookup_entry_by_path(path).ok()??;
    Some((entry.object_id(), entry.mode().value().into()))
}

fn blob_id(data: &[u8]) -> Result<ObjectId, String> {
    gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, data).map_err(message)
}

fn read(repo: &Repository, targets: &[PathBuf], capture_content: bool) -> Result<State, String> {
    read_scope(repo, Some(targets), capture_content)
}

fn read_scope(
    repo: &Repository,
    targets: Option<&[PathBuf]>,
    capture_content: bool,
) -> Result<State, String> {
    let targets: Option<HashSet<&Path>> =
        targets.map(|paths| paths.iter().map(PathBuf::as_path).collect());
    let index = repo.index()?;
    let selected = |paths: &[&Path]| {
        targets
            .as_ref()
            .is_none_or(|targets| paths.iter().any(|path| targets.contains(path)))
    };
    let conflicted = index.entries().iter().any(|entry| {
        entry.stage() != gix::index::entry::Stage::Unconflicted
            && gix::path::from_bstr(entry.path(&index)).is_ok_and(|path| selected(&[path.as_ref()]))
    });
    if conflicted {
        return Err("Resolve merge conflicts before staging or discarding files".into());
    }
    let mut changes = Vec::new();
    for status in repo.status.working_tree_status()? {
        let paths: Vec<PathBuf> = status.paths.iter().map(PathBuf::from).collect();
        if !selected(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>()) {
            continue;
        }
        let display_path = paths.last().cloned().ok_or("Git status has no file path")?;
        changes.push(Change {
            display_path,
            paths: paths.into_iter().collect(),
            staged: status.staged.is_some(),
            unstaged: status.unstaged.is_some(),
        });
    }
    changes.sort_by(|a, b| a.paths.cmp(&b.paths));
    let tree = repo.head()?;
    let mut files = Vec::new();
    for path in changes
        .iter()
        .flat_map(|c| &c.paths)
        .collect::<BTreeSet<_>>()
    {
        validate_path(&repo.workdir, path)?;
        let entry = index_entry(&index, path);
        if entry.is_some_and(|(_, mode)| mode == SUBMODULE)
            || tree
                .as_ref()
                .and_then(|t| tree_entry(t, path))
                .is_some_and(|(_, mode)| mode == SUBMODULE)
        {
            return Err("Submodule actions are not supported".into());
        }
        files.push(FileState {
            path: path.clone(),
            index: entry,
            worktree: worktree_state(&repo.workdir.join(path), capture_content)?,
        });
    }
    Ok(State {
        head: tree.map(|t| t.id),
        changes,
        files,
    })
}

fn validate_path(workdir: &Path, path: &Path) -> Result<(), String> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Git reported an invalid file path".into());
    }
    // A checkout or deletion must never follow a replaced parent symlink.
    let mut parent = path.parent();
    while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
        if workdir
            .join(p)
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(format!(
                "Cannot act on {} through a symlink directory",
                path.display()
            ));
        }
        parent = p.parent();
    }
    Ok(())
}

fn worktree_state(
    path: &Path,
    capture_content: bool,
) -> Result<Option<(u32, Option<ObjectId>)>, String> {
    let metadata = match path.symlink_metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.file_type().is_symlink() {
        let hash = if capture_content {
            let target = fs::read_link(path).map_err(|e| e.to_string())?;
            Some(blob_id(target.as_os_str().as_encoded_bytes())?)
        } else {
            None
        };
        return Ok(Some((0o120000, hash)));
    }
    if !metadata.is_file() {
        return Err(format!("Unsupported file type: {}", path.display()));
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    // Raw bytes, without checkout filters or an ODB write.
    let hash = capture_content
        .then(|| blob_id(&fs::read(path).map_err(|e| e.to_string())?))
        .transpose()?;
    Ok(Some((mode, hash)))
}

fn paths(state: &State) -> BTreeSet<PathBuf> {
    state.files.iter().map(|f| f.path.clone()).collect()
}

fn reset_index(repo: &Repository, paths: &BTreeSet<PathBuf>) -> Result<(), String> {
    let workdir = &repo.workdir;
    let tree = repo.head()?;
    let (tracked, added): (BTreeSet<_>, BTreeSet<_>) = paths
        .iter()
        .cloned()
        .partition(|p| tree.as_ref().is_some_and(|t| tree_entry(t, p).is_some()));
    git(workdir, &["restore", "--source=HEAD", "--staged"], &tracked)?;
    git(
        workdir,
        &["rm", "--cached", "--force", "--ignore-unmatch"],
        &added,
    )
}

fn apply(repo: &Repository, state: &State, kind: DiscardKind) -> Result<(), String> {
    let workdir = &repo.workdir;
    let all_paths = paths(state);
    let tree = repo.head()?;
    let index = repo.index()?;
    let (restore, remove): (BTreeSet<_>, BTreeSet<_>) =
        all_paths.into_iter().partition(|p| match kind {
            DiscardKind::All => tree.as_ref().is_some_and(|t| tree_entry(t, p).is_some()),
            DiscardKind::Unstaged => index_entry(&index, p).is_some(),
        });
    match kind {
        DiscardKind::All => {
            git(
                workdir,
                &["restore", "--source=HEAD", "--staged", "--worktree"],
                &restore,
            )?;
            git(
                workdir,
                &["rm", "--cached", "--force", "--ignore-unmatch"],
                &remove,
            )?;
        }
        DiscardKind::Unstaged => git(workdir, &["restore", "--worktree"], &restore)?,
    }
    for path in remove {
        match fs::remove_file(workdir.join(path)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn git(workdir: &Path, args: &[&str], paths: &BTreeSet<PathBuf>) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let output = Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .arg("--")
        .args(paths)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
