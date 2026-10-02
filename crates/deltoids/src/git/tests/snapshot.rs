use super::*;

fn fixture() -> (tempfile::TempDir, Repository, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    fs::write(dir.path().join("a.txt"), original()).unwrap();
    fs::write(dir.path().join("b.txt"), "different\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
    stage_all(&repo);
    commit_index(&repo, "base");
    let wrapper = Repo::discover_at(dir.path()).unwrap();
    (dir, repo, wrapper)
}

fn original() -> String {
    (0..30).map(|n| format!("line {n}\n")).collect()
}

fn assert_matches(repo: &Repo) -> WorkingTreeSnapshot {
    let snapshot = repo.working_tree_snapshot().unwrap();
    assert_eq!(snapshot.patch, repo.working_tree_diff().unwrap());
    let mut expected = repo.working_tree_status().unwrap();
    let mut actual = snapshot.stages.clone();
    expected.sort_by(|a, b| a.path.cmp(&b.path));
    actual.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(actual, expected);
    snapshot
}

#[test]
fn clean_unborn_and_detached() {
    let (dir, repo, wrapper) = fixture();
    let clean = assert_matches(&wrapper);
    assert!(clean.patch.is_empty());
    assert!(clean.stages.is_empty());
    repo.set_head_detached(repo.head().unwrap().target().unwrap())
        .unwrap();
    fs::write(dir.path().join("a.txt"), "detached edit\n").unwrap();
    assert!(!assert_matches(&wrapper).patch.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    let wrapper = Repo::discover_at(dir.path()).unwrap();
    assert_eq!(
        assert_matches(&wrapper).stages[0].unstaged,
        Some(StageChange::Untracked)
    );
    stage_all(&repo);
    assert_eq!(
        assert_matches(&wrapper).stages[0].staged,
        Some(StageChange::Added)
    );
}

#[test]
fn staging_columns_survive_net_cancellation_and_external_index_writes() {
    let (dir, _repo, wrapper) = fixture();
    fs::write(dir.path().join("a.txt"), "edit\n").unwrap();
    let unstaged = assert_matches(&wrapper);
    assert_eq!(unstaged.stages[0].staged, None);
    assert_eq!(unstaged.stages[0].unstaged, Some(StageChange::Modified));
    git_cli(dir.path(), &["add", "a.txt"]);
    let index = dir.path().join(".git/index");
    let before = fs::read(&index).unwrap();
    let modified = fs::metadata(&index).unwrap().modified().unwrap();
    let staged = assert_matches(&wrapper);
    assert_eq!(staged.patch, unstaged.patch);
    assert_eq!(staged.stages[0].staged, Some(StageChange::Modified));
    assert_eq!(staged.stages[0].unstaged, None);
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), modified);

    fs::write(dir.path().join("a.txt"), original()).unwrap();
    let cancelled = assert_matches(&wrapper);
    assert!(cancelled.patch.is_empty());
    assert_eq!(cancelled.stages[0].staged, Some(StageChange::Modified));
    assert_eq!(cancelled.stages[0].unstaged, Some(StageChange::Modified));
    git_cli(dir.path(), &["reset", "-q", "HEAD", "--", "a.txt"]);
    assert!(assert_matches(&wrapper).stages.is_empty());
}

#[test]
fn additions_deletions_and_recreation() {
    let (dir, repo, wrapper) = fixture();
    fs::create_dir(dir.path().join("ignored")).unwrap();
    fs::write(dir.path().join("ignored/file"), "ignored\n").unwrap();
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    let untracked = assert_matches(&wrapper);
    assert_eq!(untracked.stages.len(), 1);
    assert_eq!(untracked.stages[0].unstaged, Some(StageChange::Untracked));
    stage_all(&repo);
    fs::write(dir.path().join("new.txt"), "new and edited\n").unwrap();
    let edited = assert_matches(&wrapper);
    assert_eq!(edited.stages[0].staged, Some(StageChange::Added));
    assert_eq!(edited.stages[0].unstaged, Some(StageChange::Modified));
    fs::remove_file(dir.path().join("new.txt")).unwrap();
    assert_eq!(
        assert_matches(&wrapper).stages[0].unstaged,
        Some(StageChange::Deleted)
    );
    git_cli(dir.path(), &["rm", "-q", "a.txt"]);
    fs::write(dir.path().join("a.txt"), "replacement\n").unwrap();
    let recreated = assert_matches(&wrapper);
    let status = recreated.stages.iter().find(|s| s.path == "a.txt").unwrap();
    assert_eq!(status.staged, Some(StageChange::Deleted));
    assert_eq!(status.unstaged, Some(StageChange::Untracked));
}

#[test]
fn renames_in_each_column_ignore_diff_rename_configuration() {
    for config in ["true", "false", "copies"] {
        for (stage, edit) in [(false, false), (false, true), (true, false), (true, true)] {
            let (dir, repo, wrapper) = fixture();
            repo.config()
                .unwrap()
                .set_str("diff.renames", config)
                .unwrap();
            fs::rename(dir.path().join("a.txt"), dir.path().join("new.txt")).unwrap();
            if edit {
                fs::write(dir.path().join("new.txt"), original() + "edit\n").unwrap();
            }
            if stage {
                stage_all(&repo);
            }
            let snapshot = assert_matches(&wrapper);
            assert_eq!(snapshot.stages.len(), 1);
            assert_eq!(snapshot.stages[0].path, "new.txt");
            assert_eq!(
                snapshot.stages[0].staged,
                stage.then_some(StageChange::Renamed)
            );
            assert_eq!(
                snapshot.stages[0].unstaged,
                (!stage).then_some(StageChange::Renamed)
            );
        }
    }
}

#[test]
fn chained_rename_preserves_the_existing_intermediate_status_key() {
    let (dir, repo, wrapper) = fixture();
    fs::rename(dir.path().join("a.txt"), dir.path().join("middle.txt")).unwrap();
    stage_all(&repo);
    fs::rename(dir.path().join("middle.txt"), dir.path().join("final.txt")).unwrap();
    let snapshot = assert_matches(&wrapper);
    assert_eq!(snapshot.stages[0].path, "middle.txt");
    assert_eq!(snapshot.stages[0].staged, Some(StageChange::Renamed));
    assert_eq!(snapshot.stages[0].unstaged, Some(StageChange::Renamed));
}

#[test]
fn case_only_rename_uses_index_case_rules() {
    let (dir, repo, wrapper) = fixture();
    repo.config()
        .unwrap()
        .set_bool("core.ignorecase", true)
        .unwrap();
    fs::rename(dir.path().join("a.txt"), dir.path().join("temp.txt")).unwrap();
    fs::rename(dir.path().join("temp.txt"), dir.path().join("A.txt")).unwrap();
    assert_matches(&wrapper);
}

#[cfg(unix)]
#[test]
fn type_change_and_binary_content() {
    let (dir, repo, wrapper) = fixture();
    fs::remove_file(dir.path().join("a.txt")).unwrap();
    std::os::unix::fs::symlink("b.txt", dir.path().join("a.txt")).unwrap();
    assert_eq!(
        assert_matches(&wrapper).stages[0].unstaged,
        Some(StageChange::TypeChanged)
    );
    stage_all(&repo);
    assert_eq!(
        assert_matches(&wrapper).stages[0].staged,
        Some(StageChange::TypeChanged)
    );
    fs::write(dir.path().join("b.txt"), b"\0binary\xff").unwrap();
    assert!(assert_matches(&wrapper).patch.contains("Binary files"));
}

#[test]
fn submodule_and_filtered_content() {
    let dir = tempfile::tempdir().unwrap();
    setup_submodule_bump(dir.path());
    assert_matches(&Repo::discover_at(dir.path()).unwrap());
    let dir = tempfile::tempdir().unwrap();
    commit_filtered(dir.path(), "secret.txt", "hello\n");
    fs::write(dir.path().join("secret.txt"), "hello\nworld\n").unwrap();
    assert_matches(&Repo::discover_at(dir.path()).unwrap());
}

#[test]
fn conflicts_preserve_existing_column_mapping() {
    let (dir, _repo, wrapper) = fixture();
    git_cli(dir.path(), &["checkout", "-qb", "other"]);
    fs::write(dir.path().join("a.txt"), "other\n").unwrap();
    git_cli(dir.path(), &["commit", "-qam", "other"]);
    git_cli(dir.path(), &["checkout", "-q", "-"]);
    fs::write(dir.path().join("a.txt"), "main\n").unwrap();
    git_cli(dir.path(), &["commit", "-qam", "main"]);
    let merge = std::process::Command::new("git")
        .args(["merge", "other"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merge.status.success());
    assert!(assert_matches(&wrapper).stages.is_empty());
}

#[test]
fn damaged_head_is_not_an_empty_baseline() {
    let (dir, repo, wrapper) = fixture();
    let name = repo.head().unwrap().name().unwrap().to_string();
    fs::write(dir.path().join(".git").join(name), "not-an-object-id\n").unwrap();
    assert!(
        wrapper
            .working_tree_snapshot()
            .unwrap_err()
            .contains("corrupted loose reference")
    );
}

#[test]
fn many_changed_files_pair_both_columns() {
    let (dir, repo, wrapper) = fixture();
    for n in 0..1000 {
        fs::write(dir.path().join(format!("file-{n:04}")), "base\n").unwrap();
    }
    stage_all(&repo);
    commit_index(&repo, "many files");
    for n in 0..1000 {
        fs::write(dir.path().join(format!("file-{n:04}")), "base\nstaged\n").unwrap();
    }
    stage_all(&repo);
    for n in 0..1000 {
        fs::write(
            dir.path().join(format!("file-{n:04}")),
            "base\nstaged\nunstaged\n",
        )
        .unwrap();
    }
    let snapshot = assert_matches(&wrapper);
    assert_eq!(snapshot.stages.len(), 1000);
    assert!(
        snapshot
            .stages
            .iter()
            .all(|s| s.staged == Some(StageChange::Modified)
                && s.unstaged == Some(StageChange::Modified))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_paths_are_paired_before_lossy_conversion() {
    use std::os::unix::ffi::OsStrExt;
    let (dir, repo, wrapper) = fixture();
    let name = std::ffi::OsStr::from_bytes(b"bad-\xff");
    fs::write(dir.path().join(name), "base\n").unwrap();
    stage_all(&repo);
    commit_index(&repo, "byte path");
    fs::write(dir.path().join(name), "staged\n").unwrap();
    stage_all(&repo);
    fs::write(dir.path().join(name), "unstaged\n").unwrap();
    let snapshot = assert_matches(&wrapper);
    assert_eq!(snapshot.stages[0].staged, Some(StageChange::Modified));
    assert_eq!(snapshot.stages[0].unstaged, Some(StageChange::Modified));
}
