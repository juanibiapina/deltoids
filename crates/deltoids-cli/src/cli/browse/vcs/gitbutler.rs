//! The GitButler adapter. Reads the workspace through `deltoids::git`
//! like the git adapter, and asks the `but` CLI (its `--json` output) for everything
//! GitButler owns: the workspace `but status` shows, which branches
//! exist outside it, and the IDs its writes take.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use deltoids::git::{self, FileStageStatus, StageChange, WorkingTreeSnapshot};
use serde::Deserialize;

use super::{Base, Branch, OverviewContent, Stack, StatusCommit};

/// The `but` executable `Vcs` runs, looked up on `PATH`.
pub(super) const BUT: &str = "but";

/// Whether GitButler manages the repository: `but` records a target in
/// the repository's config when it adopts it, and keeps it on any branch,
/// including single-branch mode and after `but teardown`.
pub(super) fn manages(repo: &git::Repo) -> bool {
    repo.local_config_string("gitbutler.project.targetRef")
        .is_some()
}

/// The working tree as GitButler shows it: one column of uncommitted
/// changes against `HEAD`, each file's change taken against `HEAD` too.
pub(super) fn uncommitted(snapshot: WorkingTreeSnapshot) -> WorkingTreeSnapshot {
    let stages = snapshot
        .stages
        .into_iter()
        .map(|status| FileStageStatus {
            unstaged: Some(net_change(status.staged, status.unstaged)),
            staged: None,
            ..status
        })
        .collect();
    WorkingTreeSnapshot {
        patch: snapshot.patch,
        staged_patch: String::new(),
        unstaged_patch: String::new(),
        stages,
    }
}

/// A file's change from `HEAD` to the worktree, from its two git status
/// columns. A file `HEAD` lacks is added, unless the worktree lacks it
/// too: GitButler lists that leftover index entry as removed.
fn net_change(staged: Option<StageChange>, unstaged: Option<StageChange>) -> StageChange {
    use StageChange::{Added, Deleted, Modified, Renamed, TypeChanged, Untracked};
    let new_file = staged == Some(Added) || unstaged == Some(Untracked);
    let either = |change| staged == Some(change) || unstaged == Some(change);
    match (staged, unstaged) {
        (Some(Deleted), Some(Untracked)) => Modified,
        _ if new_file && unstaged == Some(Deleted) => Deleted,
        _ if new_file => Added,
        _ if either(Renamed) => Renamed,
        _ if either(Deleted) => Deleted,
        _ if either(TypeChanged) => TypeChanged,
        _ => Modified,
    }
}

#[derive(Deserialize)]
struct BranchList {
    #[serde(default)]
    branches: Vec<ListedBranch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedBranch {
    name: String,
    /// Whether a local branch exists; `false` for one only on a remote.
    #[serde(default = "local")]
    has_local: bool,
    /// Milliseconds since the epoch.
    #[serde(default)]
    last_commit_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    #[serde(default)]
    uncommitted_changes: Vec<UncommittedChange>,
    #[serde(default)]
    stacks: Vec<StatusStack>,
    #[serde(default)]
    merge_base: Option<StatusCommitJson>,
    #[serde(default)]
    upstream_state: Option<UpstreamState>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UncommittedChange {
    cli_id: String,
    file_path: String,
}

#[derive(Deserialize)]
struct StatusStack {
    #[serde(default)]
    branches: Vec<StatusBranch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusBranch {
    cli_id: String,
    name: String,
    #[serde(default)]
    commits: Vec<StatusCommitJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusCommitJson {
    #[serde(default)]
    cli_id: String,
    commit_id: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    author_name: String,
    #[serde(default)]
    author_email: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamState {
    #[serde(default)]
    behind: usize,
}

/// The workspace's branches as `but status` shows them: the applied
/// stacks with each branch's own commits, and the common base. `others`
/// is left empty.
pub(super) fn status(program: &OsStr, repo: &git::Repo) -> Result<OverviewContent, String> {
    let workdir = repo.workdir().ok_or("GitButler needs a working tree")?;
    let status: Status = parse(&run(program, workdir, &["status"])?)?;
    let stacks = status
        .stacks
        .into_iter()
        .map(|stack| Stack {
            branches: stack
                .branches
                .into_iter()
                .map(|branch| stacked(repo, branch))
                .collect(),
        })
        .collect();
    let behind = status.upstream_state.map_or(0, |state| state.behind);
    let base = status.merge_base.map(|commit| Base {
        id: commit.commit_id.clone(),
        date: commit.created_at.get(..10).unwrap_or_default().to_string(),
        summary: first_line(&commit.message),
        behind,
    });
    Ok(OverviewContent {
        stacks: Some(stacks),
        base,
        others: Vec::new(),
    })
}

/// GitButler's local branches outside the workspace, in the order `but`
/// lists them. Branches that exist only on a remote are left out.
pub(super) fn unapplied(program: &OsStr, repo: &git::Repo) -> Result<Vec<Branch>, String> {
    let workdir = repo.workdir().ok_or("GitButler needs a working tree")?;
    let list: BranchList = parse(&run(program, workdir, &["branch", "list"])?)?;
    Ok(list
        .branches
        .into_iter()
        .filter(|branch| branch.has_local)
        .map(|branch| {
            let time = branch.last_commit_at.map(|ms| ms / 1000);
            row(repo, branch.name, time, false)
        })
        .collect())
}

/// An applied branch with the commits `but status` lists for it.
fn stacked(repo: &git::Repo, branch: StatusBranch) -> Branch {
    let commits: Vec<StatusCommit> = branch.commits.into_iter().map(commit).collect();
    let mut row = match commits.first() {
        Some(tip) => Branch {
            id: None,
            rev: format!("refs/heads/{}", branch.name),
            name: branch.name,
            current: true,
            summary: tip.commit.summary.clone(),
            time: tip.commit.time,
            commits: Vec::new(),
            base: None,
        },
        None => row(repo, branch.name, None, true),
    };
    row.id = Some(branch.cli_id);
    row.commits = commits;
    row
}

fn commit(json: StatusCommitJson) -> StatusCommit {
    StatusCommit {
        id: json.cli_id,
        commit: git::Commit {
            short_id: short_id(&json.commit_id),
            id: json.commit_id,
            summary: first_line(&json.message),
            time: parse_time(&json.created_at).unwrap_or(0),
            message: json.message,
            author: json.author_name,
            email: json.author_email,
        },
    }
}

fn row(repo: &git::Repo, name: String, time: Option<i64>, current: bool) -> Branch {
    let rev = format!("refs/heads/{name}");
    let tip = repo
        .commits(&rev, 1)
        .ok()
        .and_then(|commits| commits.into_iter().next());
    Branch {
        id: None,
        summary: tip.as_ref().map(|c| c.summary.clone()).unwrap_or_default(),
        time: time.or(tip.map(|c| c.time)).unwrap_or(0),
        name,
        current,
        commits: Vec::new(),
        rev,
        base: None,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(7).collect()
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// Seconds since the epoch of a timestamp `but` writes:
/// `2026-10-13T14:28:17+00:00`, with optional fractional seconds, a `Z`
/// zone, or the `2026-10-13 14:28:17 +0200` form.
fn parse_time(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let zone = text
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit())
        .trim();
    let offset = match zone {
        "" | "Z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let digits: String = zone.chars().filter(char::is_ascii_digit).collect();
            let hours = digits.get(..2)?.parse::<i64>().ok()?;
            let minutes = digits.get(2..4).unwrap_or("0").parse::<i64>().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days from 1970-01-01 to a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Discard every change in `changes` (each one's paths, rename sources
/// included) with one `but discard`, so `but undo` reverts it at once.
/// Fails without writing when GitButler does not list one of them.
pub(super) fn discard(
    program: &OsStr,
    workdir: &Path,
    changes: &[Vec<PathBuf>],
) -> Result<(), String> {
    let status: Status = parse(&run(program, workdir, &["status"])?)?;
    let mut ids = BTreeSet::new();
    for paths in changes {
        let id = status
            .uncommitted_changes
            .iter()
            .find(|change| {
                paths
                    .iter()
                    .any(|path| Path::new(&change.file_path) == path)
            })
            .ok_or_else(|| {
                let shown = paths
                    .last()
                    .map_or(String::new(), |p| p.display().to_string());
                format!("GitButler does not list {shown} as uncommitted")
            })?;
        ids.insert(id.cli_id.as_str());
    }
    if ids.is_empty() {
        return Ok(());
    }
    let args: Vec<&str> = std::iter::once("discard").chain(ids).collect();
    run(program, workdir, &args).map(drop)
}

fn local() -> bool {
    true
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("Unexpected output from but: {e}"))
}

/// Run `but -C <workdir> --json <args>` and return its stdout.
fn run(program: &OsStr, workdir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .arg("-C")
        .arg(workdir)
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "GitButler CLI (but) not found".to_string(),
            _ => format!("Could not run but: {e}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    Err(failure(&stdout, &String::from_utf8_lossy(&output.stderr)))
}

/// The message of a failed `but` run: its JSON error, or else the first
/// line of stderr.
fn failure(stdout: &str, stderr: &str) -> String {
    #[derive(Deserialize)]
    struct Failure {
        message: String,
    }
    if let Ok(failure) = serde_json::from_str::<Failure>(stdout) {
        return failure.message;
    }
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || "but failed".to_string(),
            |line| line.trim_start_matches("Error: ").to_string(),
        )
}

#[cfg(test)]
mod tests;
