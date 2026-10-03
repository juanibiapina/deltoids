use super::*;
use tempfile::TempDir;

#[test]
#[ignore = "manual action scaling probe"]
fn action_scaling_probe() {
    for count in [1000, 4000] {
        let dir = fixture();
        let targets: Vec<_> = (0..count)
            .map(|i| PathBuf::from(format!("file{i:05}.txt")))
            .collect();
        for path in &targets {
            fs::write(dir.path().join(path), "added\n").unwrap();
        }
        let repo = open(dir.path()).unwrap();
        let start = std::time::Instant::now();
        let state = read(&repo, &targets, false).unwrap();
        eprintln!("{count} selected files: preparation={:?}", start.elapsed());
        assert_eq!(state.changes.len(), count);
    }
    let dir = fixture();
    let path = dir.path().join("large.bin");
    // Allocate outside the measurement and release the input before preparing the menu.
    fs::write(&path, vec![0; 64 * 1024 * 1024]).unwrap();
    command(dir.path(), &["add", "large.bin"]);
    let start = std::time::Instant::now();
    let choice = pending(discard(dir.path(), targets(&["large.bin"])).unwrap());
    let retained: usize = choice
        .state
        .files
        .iter()
        .filter_map(|f| f.worktree.as_ref())
        .filter_map(|(_, hash)| hash.as_ref())
        .map(std::mem::size_of_val)
        .sum();
    eprintln!(
        "64 MiB staged file: menu={:?}, retained content={retained} bytes",
        start.elapsed()
    );
}

fn command(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    command(dir.path(), &["init", "-q"]);
    command(dir.path(), &["config", "user.name", "Test"]);
    command(dir.path(), &["config", "user.email", "test@example.com"]);
    command(dir.path(), &["config", "commit.gpgsign", "false"]);
    fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    command(dir.path(), &["add", "."]);
    command(dir.path(), &["commit", "-qm", "initial"]);
    dir
}

fn targets(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

fn pending(outcome: DiscardOutcome) -> PendingDiscard {
    match outcome {
        DiscardOutcome::Choose(p) => p,
        _ => panic!("expected a discard choice without writes"),
    }
}

#[test]
fn stage_all_then_unstage_all_preserves_the_worktree() {
    let dir = fixture();
    fs::write(dir.path().join("delete.txt"), "delete\n").unwrap();
    fs::write(dir.path().join("rename.txt"), "rename\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    command(dir.path(), &["add", "."]);
    command(dir.path(), &["commit", "-qm", "more files"]);
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    fs::write(dir.path().join("a.txt"), "latest\n").unwrap();
    fs::remove_file(dir.path().join("delete.txt")).unwrap();
    fs::rename(
        dir.path().join("rename.txt"),
        dir.path().join("renamed.txt"),
    )
    .unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/new.txt"), "new\n").unwrap();
    fs::write(dir.path().join(":(glob)*.txt"), "literal\n").unwrap();
    fs::write(dir.path().join("ignored.txt"), "ignored\n").unwrap();

    assert_eq!(toggle_stage_all(dir.path()).unwrap(), "Staged all files");
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "latest\n");
    assert_eq!(command(dir.path(), &["show", ":nested/new.txt"]), "new\n");
    assert_eq!(command(dir.path(), &["show", ":renamed.txt"]), "rename\n");
    assert_eq!(command(dir.path(), &["show", "::(glob)*.txt"]), "literal\n");
    let indexed = command(dir.path(), &["ls-files"]);
    assert!(
        !indexed
            .lines()
            .any(|p| matches!(p, "delete.txt" | "rename.txt" | "ignored.txt"))
    );
    assert!(command(dir.path(), &["diff"]).is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "latest\n"
    );
    assert_eq!(toggle_stage_all(dir.path()).unwrap(), "Unstaged all files");
    assert!(command(dir.path(), &["diff", "--cached"]).is_empty());
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "original\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "latest\n"
    );
    assert!(!dir.path().join("delete.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("renamed.txt")).unwrap(),
        "rename\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("nested/new.txt")).unwrap(),
        "new\n"
    );
}

#[test]
fn stage_all_supports_clean_and_unborn_repositories_and_empty_selections_stay_empty() {
    let dir = fixture();
    assert_eq!(toggle_stage_all(dir.path()).unwrap(), "No changes");
    let dir = tempfile::tempdir().unwrap();
    command(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    toggle_stage(dir.path(), vec![]).unwrap();
    assert!(command(dir.path(), &["ls-files"]).is_empty());
    toggle_stage_all(dir.path()).unwrap();
    assert_eq!(command(dir.path(), &["show", ":new.txt"]), "new\n");
    assert_eq!(toggle_stage_all(dir.path()).unwrap(), "Unstaged all files");
    assert!(command(dir.path(), &["ls-files"]).is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("new.txt")).unwrap(),
        "new\n"
    );
}

#[test]
fn space_twice_stages_then_unstages_without_changing_worktree() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    toggle_stage(dir.path(), targets(&["a.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "edited\n");
    toggle_stage(dir.path(), targets(&["a.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "original\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[test]
fn unstaged_and_staged_discard_require_a_choice() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    assert_eq!(choice.choices(), &[DiscardKind::All, DiscardKind::Unstaged]);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
    complete_discard(choice, DiscardKind::All).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original\n"
    );
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    assert_eq!(choice.choices(), &[DiscardKind::All]);
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
    complete_discard(choice, DiscardKind::All).unwrap();
    assert!(command(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn mixed_changes_offer_both_operations_and_unstaged_discard_preserves_index() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    fs::write(dir.path().join("a.txt"), "unstaged\n").unwrap();
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    assert_eq!(choice.choices(), &[DiscardKind::All, DiscardKind::Unstaged]);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "unstaged\n"
    );
    complete_discard(choice, DiscardKind::Unstaged).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "staged\n"
    );
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    complete_discard(choice, DiscardKind::All).unwrap();
    assert!(command(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn cancelling_a_choice_does_not_write_and_changed_content_invalidates_it() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    drop(pending(discard(dir.path(), targets(&["a.txt"])).unwrap()));
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    fs::write(dir.path().join("a.txt"), "new edit\n").unwrap();
    assert!(
        complete_discard(choice, DiscardKind::All)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "new edit\n"
    );
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
}

#[test]
fn discard_menu_rejects_equal_length_edits_with_preserved_timestamps() {
    let dir = fixture();
    let path = dir.path().join("a.txt");
    fs::write(&path, "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    fs::write(&path, "before\n").unwrap();
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "after!\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        complete_discard(choice, DiscardKind::All)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "after!\n");
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
}

#[test]
fn unstaged_menu_rejects_equal_length_edits_with_preserved_timestamps() {
    let dir = fixture();
    let path = dir.path().join("a.txt");
    fs::write(&path, "before!\n").unwrap();
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "after!!\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        complete_discard(choice, DiscardKind::Unstaged)
            .unwrap_err()
            .contains("Selection changed")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "after!!\n");
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "original\n");
}

#[test]
fn deletion_and_addition_toggle_without_affecting_other_files() {
    let dir = fixture();
    fs::remove_file(dir.path().join("a.txt")).unwrap();
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    fs::write(dir.path().join("keep.txt"), "keep\n").unwrap();
    toggle_stage(dir.path(), targets(&["a.txt", "new.txt"])).unwrap();
    toggle_stage(dir.path(), targets(&["a.txt", "new.txt"])).unwrap();
    assert!(!dir.path().join("a.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("new.txt")).unwrap(),
        "new\n"
    );
    complete_discard(
        pending(discard(dir.path(), targets(&["a.txt", "new.txt"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original\n"
    );
    assert!(!dir.path().join("new.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("keep.txt")).unwrap(),
        "keep\n"
    );
}

#[test]
fn unborn_head_supports_stage_unstage_and_discard() {
    let dir = tempfile::tempdir().unwrap();
    command(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    toggle_stage(dir.path(), targets(&["new.txt"])).unwrap();
    toggle_stage(dir.path(), targets(&["new.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["ls-files"]), "");
    assert!(dir.path().join("new.txt").exists());
    toggle_stage(dir.path(), targets(&["new.txt"])).unwrap();
    let choice = pending(discard(dir.path(), targets(&["new.txt"])).unwrap());
    complete_discard(choice, DiscardKind::All).unwrap();
    assert!(!dir.path().join("new.txt").exists());
    assert!(command(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn rename_chain_can_be_staged_unstaged_and_discarded_by_its_final_name() {
    let dir = fixture();
    command(dir.path(), &["mv", "a.txt", "middle.txt"]);
    fs::rename(dir.path().join("middle.txt"), dir.path().join("final.txt")).unwrap();
    let choice = pending(discard(dir.path(), targets(&["final.txt"])).unwrap());
    assert_eq!(choice.label(), "final.txt");
    assert_eq!(choice.choices(), &[DiscardKind::All, DiscardKind::Unstaged]);
    complete_discard(choice, DiscardKind::Unstaged).unwrap();
    assert!(dir.path().join("middle.txt").exists());
    assert!(!dir.path().join("final.txt").exists());
    fs::rename(dir.path().join("middle.txt"), dir.path().join("final.txt")).unwrap();
    toggle_stage(dir.path(), targets(&["final.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["ls-files"]), "final.txt\n");
    toggle_stage(dir.path(), targets(&["final.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["ls-files"]), "a.txt\n");
    complete_discard(
        pending(discard(dir.path(), targets(&["a.txt", "final.txt"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("final.txt").exists());
}

#[test]
fn mixed_directory_selection_offers_choices_without_mutating_siblings() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
    command(dir.path(), &["add", "a.txt"]);
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    fs::write(dir.path().join("keep.txt"), "keep\n").unwrap();
    let choice = pending(discard(dir.path(), targets(&["a.txt", "new.txt"])).unwrap());
    assert_eq!(choice.choices().len(), 2);
    complete_discard(choice, DiscardKind::Unstaged).unwrap();
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "staged\n");
    assert!(!dir.path().join("new.txt").exists());
    assert!(dir.path().join("keep.txt").exists());
}

#[test]
fn literal_paths_and_binary_content_survive_staging() {
    let dir = fixture();
    let names = ["-option", ":(glob)*", "space ü.txt", "binary"];
    for name in names {
        fs::write(dir.path().join(name), [0, 255, 1]).unwrap();
    }
    toggle_stage(dir.path(), targets(&names)).unwrap();
    assert_eq!(
        command(dir.path(), &["ls-files", "-z"])
            .split('\0')
            .filter(|s| !s.is_empty())
            .count(),
        5
    );
    toggle_stage(dir.path(), targets(&names)).unwrap();
    assert_eq!(command(dir.path(), &["ls-files"]), "a.txt\n");
    complete_discard(
        pending(discard(dir.path(), targets(&names)).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    for name in names {
        assert!(!dir.path().join(name).exists());
    }
}

#[test]
fn clean_selection_has_no_available_discard_actions() {
    let dir = fixture();
    let choice = pending(discard(dir.path(), targets(&["a.txt"])).unwrap());
    assert!(choice.choices().is_empty());
    assert!(
        complete_discard(choice, DiscardKind::All)
            .unwrap_err()
            .contains("unavailable")
    );
}

#[cfg(unix)]
#[test]
fn symlink_discard_never_follows_the_target_and_restores_type_changes() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("target"), "keep\n").unwrap();
    fs::remove_file(dir.path().join("a.txt")).unwrap();
    symlink(external.path().join("target"), dir.path().join("a.txt")).unwrap();
    complete_discard(
        pending(discard(dir.path(), targets(&["a.txt"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert!(
        !dir.path()
            .join("a.txt")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original\n"
    );
    symlink(external.path().join("target"), dir.path().join("new-link")).unwrap();
    complete_discard(
        pending(discard(dir.path(), targets(&["new-link"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(external.path().join("target")).unwrap(),
        "keep\n"
    );
}

#[test]
fn a_locked_index_reports_failure_without_changing_content() {
    let dir = fixture();
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
    fs::write(dir.path().join(".git/index.lock"), "locked").unwrap();
    assert!(
        toggle_stage(dir.path(), targets(&["a.txt"]))
            .unwrap_err()
            .contains("index.lock")
    );
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "original\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "edited\n"
    );
    fs::remove_file(dir.path().join(".git/index.lock")).unwrap();
    toggle_stage(dir.path(), targets(&["a.txt"])).unwrap();
    assert_eq!(command(dir.path(), &["show", ":a.txt"]), "edited\n");
}

#[test]
fn submodules_reject_the_complete_selection_before_deleting_other_files() {
    let dir = fixture();
    let oid = command(dir.path(), &["rev-parse", "HEAD"]);
    command(
        dir.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},module", oid.trim()),
        ],
    );
    fs::create_dir(dir.path().join("module")).unwrap();
    fs::write(dir.path().join("other.txt"), "keep\n").unwrap();
    assert!(discard(dir.path(), targets(&["module", "other.txt"])).is_err());
    assert!(toggle_stage(dir.path(), targets(&["module", "other.txt"])).is_err());
    assert!(toggle_stage_all(dir.path()).is_err());
    assert!(
        !command(dir.path(), &["ls-files"])
            .lines()
            .any(|p| p == "other.txt")
    );
    assert!(dir.path().join("other.txt").exists());
}

#[cfg(unix)]
#[test]
fn discard_restores_executable_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = fixture();
    command(dir.path(), &["config", "core.filemode", "true"]);
    let path = dir.path().join("a.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    complete_discard(
        pending(discard(dir.path(), targets(&["a.txt"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o111, 0);
}

#[test]
fn discard_uses_git_checkout_filters() {
    let dir = fixture();
    command(
        dir.path(),
        &["config", "filter.words.clean", "sed s/visible/stored/g"],
    );
    command(
        dir.path(),
        &["config", "filter.words.smudge", "sed s/stored/visible/g"],
    );
    fs::write(
        dir.path().join(".gitattributes"),
        "filtered.txt filter=words\n",
    )
    .unwrap();
    fs::write(dir.path().join("filtered.txt"), "visible original\n").unwrap();
    command(dir.path(), &["add", "."]);
    command(dir.path(), &["commit", "-qm", "filtered fixture"]);
    fs::write(dir.path().join("filtered.txt"), "visible edit\n").unwrap();
    complete_discard(
        pending(discard(dir.path(), targets(&["filtered.txt"])).unwrap()),
        DiscardKind::All,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("filtered.txt")).unwrap(),
        "visible original\n"
    );
    assert_eq!(
        command(dir.path(), &["show", ":filtered.txt"]),
        "stored original\n"
    );
}

#[test]
fn conflicts_reject_the_complete_selection_before_any_writes() {
    let dir = fixture();
    let repo = open(dir.path()).unwrap();
    let mut index = repo.index().unwrap();
    let entries: Vec<_> = (1..=3)
        .map(|_| index.get_path(Path::new("a.txt"), 0).unwrap())
        .collect();
    index.remove_path(Path::new("a.txt")).unwrap();
    for (stage, mut entry) in (1..=3).zip(entries) {
        entry.flags = (entry.flags & !0x3000) | (stage << 12);
        index.add(&entry).unwrap();
    }
    index.write().unwrap();
    fs::write(dir.path().join("other.txt"), "keep\n").unwrap();
    assert!(discard(dir.path(), targets(&["other.txt", "a.txt"])).is_err());
    assert!(toggle_stage(dir.path(), targets(&["other.txt", "a.txt"])).is_err());
    assert!(toggle_stage_all(dir.path()).is_err());
    assert!(
        !command(dir.path(), &["ls-files"])
            .lines()
            .any(|p| p == "other.txt")
    );
    assert!(dir.path().join("other.txt").exists());
    assert!(repo.index().unwrap().has_conflicts());
}
