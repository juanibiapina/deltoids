use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use git2::{Repository, Status, StatusOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscardKind {
    All,
    Unstaged,
}

impl DiscardKind {
    pub(super) const ALL: [Self; 2] = [Self::All, Self::Unstaged];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::All => "Discard all changes",
            Self::Unstaged => "Discard unstaged changes",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::All => "Restore HEAD, including staged changes. Delete added files.",
            Self::Unstaged => "Restore the index. Keep staged changes. Delete untracked files.",
        }
    }
}

pub(super) enum DiscardOutcome {
    Applied(String),
    Choose(PendingDiscard),
}

#[derive(Debug)]
pub(super) struct PendingDiscard {
    workdir: PathBuf,
    targets: Vec<PathBuf>,
    state: State,
    choices: Vec<DiscardKind>,
}

impl PendingDiscard {
    #[cfg(test)]
    pub(super) fn choices(&self) -> &[DiscardKind] {
        &self.choices
    }

    pub(super) fn is_available(&self, kind: DiscardKind) -> bool {
        self.choices.contains(&kind)
    }

    pub(super) fn label(&self) -> String {
        if self.state.changes.len() == 1 {
            self.state.changes[0].display_path.display().to_string()
        } else {
            format!("{} selected files", self.state.changes.len())
        }
    }
}

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
    index: Option<(git2::Oid, u32)>,
    worktree: Option<(u32, Option<git2::Oid>)>,
}

#[derive(Debug, PartialEq, Eq)]
struct State {
    head: Option<git2::Oid>,
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
        let index = repo.index().map_err(message)?;
        let paths: BTreeSet<_> = state
            .changes
            .iter()
            .filter(|c| c.unstaged)
            .flat_map(|c| c.paths.iter())
            .filter(|p| {
                index.get_path(p, 0).is_some() || workdir.join(p).symlink_metadata().is_ok()
            })
            .cloned()
            .collect();
        git(workdir, &["add", "-A"], &paths)?;
        Ok("Staged selection".into())
    } else {
        let paths = paths(&state);
        reset_index(&repo, &paths)?;
        Ok("Unstaged selection".into())
    }
}

pub(super) fn discard(workdir: &Path, targets: Vec<PathBuf>) -> Result<DiscardOutcome, String> {
    let repo = open(workdir)?;
    let state = read(&repo, &targets, true)?;
    let mut choices = Vec::new();
    if !state.changes.is_empty() {
        choices.push(DiscardKind::All);
    }
    if state.changes.iter().any(|c| c.unstaged) {
        choices.push(DiscardKind::Unstaged);
    }
    Ok(DiscardOutcome::Choose(PendingDiscard {
        workdir: workdir.to_path_buf(),
        targets,
        state,
        choices,
    }))
}

pub(super) fn complete_discard(
    pending: PendingDiscard,
    kind: DiscardKind,
) -> Result<String, String> {
    if !pending.is_available(kind) {
        return Err("This discard action is unavailable".into());
    }
    let repo = open(&pending.workdir)?;
    if read(&repo, &pending.targets, true)? != pending.state {
        return Err("Selection changed; press d again to review the current changes".into());
    }
    apply(&repo, &pending.state, kind)?;
    Ok(match kind {
        DiscardKind::All => "Discarded all selected changes",
        DiscardKind::Unstaged => "Discarded unstaged changes",
    }
    .into())
}

fn open(workdir: &Path) -> Result<Repository, String> {
    Repository::open(workdir).map_err(message)
}

fn head(repo: &Repository) -> Result<Option<git2::Tree<'_>>, String> {
    match repo.head() {
        Ok(reference) => reference.peel_to_tree().map(Some).map_err(message),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(message(e)),
    }
}

fn read(repo: &Repository, targets: &[PathBuf], capture_content: bool) -> Result<State, String> {
    let targets: HashSet<&Path> = targets.iter().map(PathBuf::as_path).collect();
    let workdir = repo.workdir().ok_or("Git actions require a working tree")?;
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let statuses = repo.statuses(Some(&mut options)).map_err(message)?;
    let mut changes = Vec::new();
    for entry in statuses.iter() {
        let deltas = [entry.head_to_index(), entry.index_to_workdir()];
        if !deltas
            .iter()
            .flatten()
            .flat_map(|delta| [delta.old_file(), delta.new_file()])
            .filter_map(|file| file.path())
            .any(|path| targets.contains(path))
        {
            continue;
        }
        let names: BTreeSet<_> = deltas
            .iter()
            .flatten()
            .flat_map(|delta| [delta.old_file(), delta.new_file()])
            .filter_map(|file| file.path().map(PathBuf::from))
            .collect();
        let submodule = deltas
            .iter()
            .flatten()
            .flat_map(|delta| [delta.old_file(), delta.new_file()])
            .any(|file| file.mode() == git2::FileMode::Commit);
        let flags = entry.status();
        if flags.contains(Status::CONFLICTED) {
            return Err(
                "Resolve merge conflicts before staging or discarding this selection".into(),
            );
        }
        if submodule {
            return Err("Submodule actions are not supported".into());
        }
        let display_path = deltas
            .iter()
            .flatten()
            .flat_map(|delta| [delta.old_file(), delta.new_file()])
            .filter_map(|file| file.path().map(PathBuf::from))
            .next_back()
            .ok_or("Git status has no file path")?;
        changes.push(Change {
            display_path,
            paths: names,
            staged: flags.intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE,
            ),
            unstaged: flags.intersects(
                Status::WT_NEW
                    | Status::WT_MODIFIED
                    | Status::WT_DELETED
                    | Status::WT_RENAMED
                    | Status::WT_TYPECHANGE,
            ),
        });
    }
    changes.sort_by(|a, b| a.paths.cmp(&b.paths));
    let index = repo.index().map_err(message)?;
    let tree = head(repo)?;
    let mut files = Vec::new();
    for path in changes
        .iter()
        .flat_map(|c| &c.paths)
        .collect::<BTreeSet<_>>()
    {
        validate_path(workdir, path)?;
        let entry = index.get_path(path, 0);
        if entry.as_ref().is_some_and(|e| e.mode == 0o160000)
            || tree
                .as_ref()
                .and_then(|t| t.get_path(path).ok())
                .is_some_and(|e| e.filemode() == 0o160000)
        {
            return Err("Submodule actions are not supported".into());
        }
        files.push(FileState {
            path: path.clone(),
            index: entry.map(|e| (e.id, e.mode)),
            worktree: worktree_state(&workdir.join(path), capture_content)?,
        });
    }
    Ok(State {
        head: tree.map(|t| t.id()),
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
) -> Result<Option<(u32, Option<git2::Oid>)>, String> {
    let metadata = match path.symlink_metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.file_type().is_symlink() {
        let hash = if capture_content {
            let target = fs::read_link(path).map_err(|e| e.to_string())?;
            Some(
                git2::Oid::hash_object(
                    git2::ObjectType::Blob,
                    target.as_os_str().as_encoded_bytes(),
                )
                .map_err(message)?,
            )
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
    // hash_file streams raw bytes without checkout filters or an ODB write.
    let hash = capture_content
        .then(|| git2::Oid::hash_file(git2::ObjectType::Blob, path))
        .transpose()
        .map_err(message)?;
    Ok(Some((mode, hash)))
}

fn paths(state: &State) -> BTreeSet<PathBuf> {
    state.files.iter().map(|f| f.path.clone()).collect()
}

fn reset_index(repo: &Repository, paths: &BTreeSet<PathBuf>) -> Result<(), String> {
    let workdir = repo.workdir().ok_or("Git actions require a working tree")?;
    let tree = head(repo)?;
    let (tracked, added): (BTreeSet<_>, BTreeSet<_>) = paths
        .iter()
        .cloned()
        .partition(|p| tree.as_ref().is_some_and(|t| t.get_path(p).is_ok()));
    git(workdir, &["restore", "--source=HEAD", "--staged"], &tracked)?;
    git(
        workdir,
        &["rm", "--cached", "--force", "--ignore-unmatch"],
        &added,
    )
}

fn apply(repo: &Repository, state: &State, kind: DiscardKind) -> Result<(), String> {
    let workdir = repo.workdir().ok_or("Git actions require a working tree")?;
    let all_paths = paths(state);
    let tree = head(repo)?;
    let index = repo.index().map_err(message)?;
    let (restore, remove): (BTreeSet<_>, BTreeSet<_>) =
        all_paths.into_iter().partition(|p| match kind {
            DiscardKind::All => tree.as_ref().is_some_and(|t| t.get_path(p).is_ok()),
            DiscardKind::Unstaged => index.get_path(p, 0).is_some(),
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

fn message(error: git2::Error) -> String {
    error.message().to_string()
}

#[cfg(test)]
mod tests;
