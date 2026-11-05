use std::fs;
use std::path::{Path, PathBuf};

use deltoids::git::StageChange;
use tempfile::TempDir;

#[cfg(unix)]
use super::super::test_support::stub_but;
use super::super::test_support::{adopt, but_log, fixture};
use super::super::{Base, Columns, DiscardKind, Vcs, Writer};
use crate::test_git::TestRepo;

fn commit_all(repo: &TestRepo, message: &str) {
    repo.stage_all();
    repo.commit(message);
}

/// A repository with one commit holding `files`.
fn repo_with(files: &[&str]) -> (TempDir, TestRepo) {
    let dir = tempfile::tempdir().unwrap();
    let repo = TestRepo::init(dir.path());
    for file in files {
        fs::write(dir.path().join(file), format!("{file}\n")).unwrap();
    }
    commit_all(&repo, "initial");
    (dir, repo)
}

/// Move `HEAD` onto a GitButler workspace branch at the current commit,
/// as `but` does once it applies a second branch.
fn enter_workspace(repo: &TestRepo) {
    adopt(repo.dir());
    move_to_workspace_branch(repo);
}

fn move_to_workspace_branch(repo: &TestRepo) {
    repo.git(&["branch", "-f", "gitbutler/workspace"]);
    repo.set_head("gitbutler/workspace");
}

fn stage(repo: &TestRepo, path: &str) {
    repo.git(&["add", "--", path]);
}

#[test]
fn a_workspace_shows_one_column_of_changes_against_head() {
    let (dir, repo) = repo_with(&["modified.txt", "renamed.txt"]);
    enter_workspace(&repo);
    fs::write(dir.path().join("modified.txt"), "edit\n").unwrap();
    fs::write(dir.path().join("untracked.txt"), "new\n").unwrap();
    fs::write(dir.path().join("staged-new.txt"), "staged\n").unwrap();
    stage(&repo, "staged-new.txt");
    fs::write(dir.path().join("staged-new.txt"), "staged then edited\n").unwrap();
    fs::write(dir.path().join("gone.txt"), "gone\n").unwrap();
    stage(&repo, "gone.txt");
    fs::remove_file(dir.path().join("gone.txt")).unwrap();
    fs::rename(dir.path().join("renamed.txt"), dir.path().join("moved.txt")).unwrap();
    repo.git(&["add", "--", "renamed.txt", "moved.txt"]);

    let working = Vcs::discover_at(dir.path())
        .unwrap()
        .working_tree()
        .unwrap();

    assert_eq!(working.columns, Columns::Single);
    assert!(working.snapshot.staged_patch.is_empty());
    assert!(working.snapshot.unstaged_patch.is_empty());
    let mut kinds: Vec<_> = working
        .snapshot
        .stages
        .iter()
        .map(|status| (status.path.as_str(), status.staged, status.unstaged))
        .collect();
    kinds.sort_by_key(|(path, _, _)| *path);
    assert_eq!(
        kinds,
        [
            ("gone.txt", None, Some(StageChange::Deleted)),
            ("modified.txt", None, Some(StageChange::Modified)),
            ("moved.txt", None, Some(StageChange::Renamed)),
            ("staged-new.txt", None, Some(StageChange::Added)),
            ("untracked.txt", None, Some(StageChange::Added)),
        ]
    );
}

#[test]
fn a_single_branch_repository_but_adopted_shows_one_column() {
    let (dir, _repo) = repo_with(&["a.txt"]);
    adopt(dir.path());
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let working = Vcs::discover_at(dir.path())
        .unwrap()
        .working_tree()
        .unwrap();
    assert_eq!(working.columns, Columns::Single);
}

#[test]
fn a_workspace_branch_without_a_gitbutler_target_is_plain_git() {
    let (dir, repo) = repo_with(&["a.txt"]);
    move_to_workspace_branch(&repo);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let working = Vcs::discover_at(dir.path())
        .unwrap()
        .working_tree()
        .unwrap();
    assert_eq!(working.columns, Columns::Split);
}

#[test]
fn a_plain_repository_keeps_staged_and_unstaged_columns() {
    let (dir, _repo) = repo_with(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let working = Vcs::discover_at(dir.path())
        .unwrap()
        .working_tree()
        .unwrap();
    assert_eq!(working.columns, Columns::Split);
}

#[test]
fn staging_in_a_workspace_leaves_the_index_alone() {
    let (dir, repo) = repo_with(&["a.txt"]);
    enter_workspace(&repo);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    let index_before = fs::read(repo.git_dir().join("index")).unwrap();
    let writer = Writer::at(dir.path());

    let one = writer.toggle_stage(vec![PathBuf::from("a.txt")]).unwrap();
    let all = writer.toggle_stage_all().unwrap();

    assert!(one.contains("no staging"), "{one}");
    assert!(all.contains("no staging"), "{all}");
    assert_eq!(
        fs::read(repo.git_dir().join("index")).unwrap(),
        index_before
    );
}

#[test]
fn a_workspace_offers_one_discard() {
    let (dir, repo) = repo_with(&["a.txt"]);
    enter_workspace(&repo);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let vcs = Vcs::discover_at(dir.path()).unwrap();

    let placeholders = Vcs::discard_placeholders(Some(&vcs));
    let pending = Writer::at(dir.path())
        .discard(vec![PathBuf::from("a.txt")])
        .unwrap();

    assert_eq!(placeholders.len(), 1);
    assert!(!placeholders[0].available);
    let choices = pending.choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].kind, DiscardKind::All);
    assert!(choices[0].available);
    assert!(choices[0].description.contains("but undo"));
}

#[test]
fn a_discard_read_before_entering_a_workspace_is_refused() {
    let (dir, repo) = repo_with(&["a.txt"]);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let writer = Writer::at(dir.path());
    let pending = writer.discard(vec![PathBuf::from("a.txt")]).unwrap();
    enter_workspace(&repo);

    let error = writer
        .complete_discard(pending, DiscardKind::All)
        .unwrap_err();

    assert!(error.contains("repository changed"), "{error}");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edit\n"
    );
}

/// The refs `but-status-stacks.json` and `but-branch-list.json` name:
/// four applied branches, a parked local one, and one that only exists
/// on `origin`.
fn workspace_with_branches() -> (TempDir, TestRepo) {
    let (dir, repo) = repo_with(&["a.txt"]);
    for name in ["empty", "top-feature", "base-feature", "side", "parked"] {
        repo.git(&["branch", name]);
    }
    repo.git(&["remote", "add", "origin", "https://example.com/repo.git"]);
    let head = repo.rev("HEAD");
    repo.git(&["update-ref", "refs/remotes/origin/remote-only", &head]);
    enter_workspace(&repo);
    (dir, repo)
}

#[cfg(unix)]
fn stubbed(dir: &Path, stubs: &Path, status: &str) -> Vcs {
    Vcs::discover_at(dir)
        .unwrap()
        .with_but(stub_but(stubs, &fixture(status)).into_os_string())
}

#[cfg(unix)]
#[test]
fn a_workspace_overview_holds_what_but_status_lists() {
    let (dir, _repo) = workspace_with_branches();
    let stubs = tempfile::tempdir().unwrap();
    let mut vcs = stubbed(dir.path(), stubs.path(), "but-status-stacks.json");

    let content = vcs.overview().unwrap();

    assert_eq!(content.others_heading, "unapplied");
    let stacks = &content.stacks;
    let names: Vec<Vec<(Option<&str>, &str)>> = stacks
        .iter()
        .map(|stack| {
            stack
                .branches
                .iter()
                .map(|b| (b.id.as_deref(), b.name.as_str()))
                .collect()
        })
        .collect();
    assert_eq!(
        names,
        [
            vec![(Some("em"), "empty")],
            vec![(Some("to"), "top-feature"), (Some("as"), "base-feature")],
            vec![(Some("si"), "side")],
        ]
    );
    let side = &stacks[2].branches[0];
    let commits: Vec<_> = side
        .commits
        .iter()
        .map(|c| (c.id.as_str(), c.commit.summary.as_str()))
        .collect();
    assert_eq!(
        commits,
        [
            ("wtux", "unpushed"),
            ("sqlz", "after push"),
            ("pprv", "more side"),
            ("wlrt", "side work"),
        ]
    );
    assert_eq!(side.commits[1].commit.time, 1_791_990_162);
    assert_eq!(side.summary, "unpushed");
    let empty = &stacks[0].branches[0];
    assert!(empty.commits.is_empty());
    assert_eq!(empty.summary, "initial", "an empty branch reads its tip");
    assert!(stacks.iter().flat_map(|s| &s.branches).all(|b| b.current));
    let others: Vec<_> = content.others.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(others, ["parked"]);
    assert_eq!(vcs.commits(&content.others[0], 10).unwrap().len(), 1);
    assert_eq!(
        content.base,
        Some(Base {
            id: "3917b63b1afceb59f7c748f6bfe40c5c8bceeb93".into(),
            target: Some("origin/main".into()),
            date: "2026-10-14".into(),
            summary: "init".into(),
            behind: 0,
        })
    );
}

#[cfg(unix)]
#[test]
fn the_base_carries_upstream_lag() {
    let (dir, _repo) = workspace_with_branches();
    let stubs = tempfile::tempdir().unwrap();
    let mut vcs = stubbed(dir.path(), stubs.path(), "but-status-upstream.json");

    let base = vcs.overview().unwrap().base.unwrap();

    assert_eq!(base.behind, 1);
}

#[cfg(unix)]
#[test]
fn the_workspace_runs_but_again_only_after_a_ref_moves() {
    let (dir, repo) = workspace_with_branches();
    let stubs = tempfile::tempdir().unwrap();
    let mut vcs = stubbed(dir.path(), stubs.path(), "but-status-stacks.json");

    let _ = vcs.overview();
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let _ = vcs.overview();
    assert_eq!(but_log(stubs.path()), ["status", "branch list"]);

    repo.git(&["branch", "new-branch"]);
    let _ = vcs.overview();
    assert_eq!(
        but_log(stubs.path()),
        ["status", "branch list", "status", "branch list"]
    );
}

#[cfg(unix)]
#[test]
fn the_workspace_is_read_again_after_gitbutler_lets_go_and_adopts_again() {
    let (dir, _repo) = repo_with(&["a.txt"]);
    adopt(dir.path());
    let stubs = tempfile::tempdir().unwrap();
    let mut vcs = stubbed(dir.path(), stubs.path(), "but-status.json");
    let _ = vcs.overview();

    TestRepo::open(dir.path()).git(&["config", "--unset", "gitbutler.project.targetRef"]);
    assert_eq!(vcs.overview().unwrap().others_heading, "branches");
    adopt(dir.path());
    let _ = vcs.overview();

    assert_eq!(
        but_log(stubs.path()),
        ["status", "branch list", "status", "branch list"]
    );
}

#[test]
fn a_workspace_without_but_reports_it_in_place_of_branches() {
    let (dir, repo) = repo_with(&["a.txt"]);
    enter_workspace(&repo);
    let mut vcs = Vcs::discover_at(dir.path()).unwrap();
    vcs.but = "/nonexistent/but".into();

    let error = vcs.overview().unwrap_err();

    assert!(error.contains("not found"), "{error}");
}

#[test]
fn a_plain_repository_lists_local_branches() {
    let (dir, repo) = repo_with(&["a.txt"]);
    repo.git(&["branch", "other"]);
    let mut vcs = Vcs::discover_at(dir.path()).unwrap();
    vcs.but = "/nonexistent/but".into();

    let content = vcs.overview().unwrap();

    assert_eq!(content.others_heading, "branches");
    assert!(content.stacks.is_empty(), "no base, so no stack");
    assert_eq!(content.base, None);
    assert_eq!(content.others.len(), 2);
    assert!(content.others[0].current);
    assert_eq!(vcs.commits(&content.others[1], 10).unwrap().len(), 1);
}

#[test]
fn a_plain_repository_stacks_the_checked_out_branch_on_its_base() {
    let (dir, repo) = repo_with(&["a.txt"]);
    let base = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let main = repo.git(&["branch", "--show-current"]).trim().to_string();
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    fs::write(dir.path().join("f.txt"), "f\n").unwrap();
    repo.git(&["add", "f.txt"]);
    repo.git(&["commit", "-q", "-m", "feature work"]);
    let mut vcs = Vcs::discover_at(dir.path()).unwrap();

    let content = vcs.overview().unwrap();

    let [stack] = content.stacks.as_slice() else {
        panic!("one stack: {:?}", content.stacks);
    };
    let [feature] = stack.branches.as_slice() else {
        panic!("one branch");
    };
    assert_eq!(feature.name, "feature");
    let commits: Vec<_> = feature
        .commits
        .iter()
        .map(|c| c.commit.summary.as_str())
        .collect();
    assert_eq!(commits, ["feature work"]);
    assert_eq!(feature.commits[0].id.len(), 7);
    let base_row = content.base.unwrap();
    assert_eq!(base_row.id, base);
    assert_eq!(base_row.target.as_deref(), Some("origin/main"));
    let others: Vec<_> = content.others.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(others, [main.as_str()]);
}

/// The files `but-status.json` lists: `a.txt` modified, `new.txt`
/// added, and `ren.txt` renamed to `ren2.txt`.
#[cfg(unix)]
fn workspace_with_uncommitted_changes() -> TempDir {
    let (dir, repo) = repo_with(&["a.txt", "ren.txt", "kept.txt"]);
    enter_workspace(&repo);
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    fs::rename(dir.path().join("ren.txt"), dir.path().join("ren2.txt")).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn discard_in_a_workspace_runs_but_discard_with_the_selections_ids() {
    let dir = workspace_with_uncommitted_changes();
    let stubs = tempfile::tempdir().unwrap();
    let writer = Writer {
        workdir: dir.path().to_path_buf(),
        but: stub_but(stubs.path(), &fixture("but-status.json")).into_os_string(),
    };
    let targets = ["a.txt", "ren.txt", "ren2.txt"].map(PathBuf::from).to_vec();
    let pending = writer.discard(targets).unwrap();

    let message = writer.complete_discard(pending, DiscardKind::All).unwrap();

    assert!(message.contains("but undo"), "{message}");
    assert_eq!(but_log(stubs.path()), ["status", "discard nkns svvx"]);
}

#[cfg(unix)]
#[test]
fn discard_in_a_workspace_refuses_a_file_gitbutler_does_not_list() {
    let dir = workspace_with_uncommitted_changes();
    fs::write(dir.path().join("kept.txt"), "edit\n").unwrap();
    let stubs = tempfile::tempdir().unwrap();
    let writer = Writer {
        workdir: dir.path().to_path_buf(),
        but: stub_but(stubs.path(), &fixture("but-status.json")).into_os_string(),
    };
    let targets = ["a.txt", "kept.txt"].map(PathBuf::from).to_vec();
    let pending = writer.discard(targets).unwrap();

    let error = writer
        .complete_discard(pending, DiscardKind::All)
        .unwrap_err();

    assert!(error.contains("kept.txt"), "{error}");
    assert_eq!(but_log(stubs.path()), ["status"]);
}
