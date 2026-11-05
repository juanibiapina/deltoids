use super::*;
/// Run `git args` as Ada Lovelace at `seconds` since the epoch.
fn as_ada(repo: &Fixture, seconds: i64, args: &[&str], input: &[u8]) -> String {
    let date = format!("@{seconds} +0000");
    repo.git_with(
        args,
        input,
        &[
            ("GIT_AUTHOR_NAME", "Ada Lovelace"),
            ("GIT_AUTHOR_EMAIL", "ada@example.com"),
            ("GIT_COMMITTER_NAME", "Ada Lovelace"),
            ("GIT_COMMITTER_EMAIL", "ada@example.com"),
            ("GIT_AUTHOR_DATE", &date),
            ("GIT_COMMITTER_DATE", &date),
        ],
    )
}

/// Write `files` into the working tree, stage them, and commit on `HEAD`
/// at `seconds` since the epoch, so recency ordering is deterministic.
fn commit_at(dir: &Path, repo: &Fixture, files: &[(&str, &str)], msg: &str, seconds: i64) {
    for (path, text) in files {
        fs::write(dir.join(path), text).unwrap();
    }
    stage_all(repo);
    let args = [
        "commit",
        "-q",
        "--allow-empty",
        "--cleanup=verbatim",
        "-m",
        msg,
    ];
    as_ada(repo, seconds, &args, b"");
}

/// Create branch `name` at `HEAD` and point `HEAD` at it, leaving the
/// index and working tree alone.
fn checkout_new_branch(repo: &Fixture, name: &str) {
    repo.git(&["branch", name]);
    set_head(repo, name);
}

fn set_head(repo: &Fixture, branch: &str) {
    repo.git(&["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")]);
}

#[test]
fn local_branches_list_the_checked_out_branch_first_then_by_recency() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "first", 100);
    let main = raw.branch();
    checkout_new_branch(&raw, "older");
    commit_at(dir.path(), &raw, &[("a.txt", "b\n")], "older work", 200);
    set_head(&raw, &main);
    checkout_new_branch(&raw, "newer");
    commit_at(dir.path(), &raw, &[("a.txt", "c\n")], "newer work", 300);
    set_head(&raw, &main);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let branches = repo.local_branches().unwrap();

    let names: Vec<_> = branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, [main.as_str(), "newer", "older"]);
    assert!(branches[0].head);
    assert!(!branches[1].head);
    assert_eq!(branches[1].summary, "newer work");
    assert_eq!(branches[1].time, 300);
}

#[test]
fn local_branches_leave_out_remote_tracking_branches() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "first", 100);
    let head = raw.rev("HEAD");
    raw.git(&["update-ref", "refs/remotes/origin/feature", &head]);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let branches = repo.local_branches().unwrap();

    assert_eq!(branches.len(), 1);
}

#[test]
fn local_branches_of_an_unborn_repo_are_empty() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    let repo = Repo::discover_at(dir.path()).unwrap();

    assert_eq!(repo.local_branches().unwrap(), Vec::new());
}

#[test]
fn commits_walk_a_branch_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "first", 100);
    commit_at(
        dir.path(),
        &raw,
        &[("a.txt", "b\n")],
        "second\n\nWith a body.\n",
        200,
    );
    let branch = raw.branch();

    let repo = Repo::discover_at(dir.path()).unwrap();
    let commits = repo.commits(&branch, 10).unwrap();

    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["second", "first"]);
    let newest = &commits[0];
    assert_eq!(newest.id.len(), 40);
    assert_eq!(newest.short_id, newest.id[..7]);
    assert_eq!(newest.message, "second\n\nWith a body.\n");
    assert_eq!(newest.author, "Ada Lovelace");
    assert_eq!(newest.email, "ada@example.com");
    assert_eq!(newest.time, 200);
}

#[test]
fn commits_stop_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    for n in 0..5 {
        commit_at(
            dir.path(),
            &raw,
            &[("a.txt", &format!("{n}\n"))],
            &format!("c{n}"),
            100 + n,
        );
    }
    let branch = raw.branch();

    let repo = Repo::discover_at(dir.path()).unwrap();
    let commits = repo.commits(&branch, 2).unwrap();

    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["c4", "c3"]);
}

/// Commit `HEAD`'s tree on `parents` at `seconds`, moving no ref.
fn commit_on(repo: &Fixture, parents: &[&str], msg: &str, seconds: i64) -> String {
    let mut args = vec!["commit-tree", "HEAD^{tree}", "-m", msg];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    as_ada(repo, seconds, &args, b"")
}

/// A branch `merged` whose two lines of work interleave in time:
/// `base` 100, `side 1` 200, `main 2` 300, `side 3` 400, `main 4` 500,
/// and the merge 600.
fn interleaved_merge(dir: &Path) -> Fixture {
    let raw = init_repo(dir);
    commit_at(dir, &raw, &[("a.txt", "a\n")], "base", 100);
    let base = raw.rev("HEAD");
    let side1 = commit_on(&raw, &[&base], "side 1", 200);
    let main2 = commit_on(&raw, &[&base], "main 2", 300);
    let side3 = commit_on(&raw, &[&side1], "side 3", 400);
    let main4 = commit_on(&raw, &[&main2], "main 4", 500);
    let merge = commit_on(&raw, &[&main4, &side3], "merge", 600);
    raw.git(&["update-ref", "refs/heads/merged", &merge]);
    raw
}

#[test]
fn commits_list_merged_history_newest_first_each_once() {
    let dir = tempfile::tempdir().unwrap();
    interleaved_merge(dir.path());

    let repo = Repo::discover_at(dir.path()).unwrap();
    let commits = repo.commits("merged", 10).unwrap();

    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        ["merge", "main 4", "side 3", "main 2", "side 1", "base"]
    );
}

#[test]
fn commits_stop_at_the_limit_across_merges() {
    let dir = tempfile::tempdir().unwrap();
    interleaved_merge(dir.path());

    let repo = Repo::discover_at(dir.path()).unwrap();
    let commits = repo.commits("merged", 3).unwrap();

    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["merge", "main 4", "side 3"]);
}

#[test]
fn commits_of_an_unknown_branch_are_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "first", 100);

    let repo = Repo::discover_at(dir.path()).unwrap();

    assert!(repo.commits("nope", 10).is_err());
}

#[test]
fn commit_patch_diffs_against_the_first_parent_with_full_blob_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "one\ntwo\n")], "first", 100);
    commit_at(
        dir.path(),
        &raw,
        &[("a.txt", "one\nTWO\n"), ("b.txt", "new\n")],
        "second",
        200,
    );
    let id = raw.rev("HEAD");
    // Uncommitted edits never leak into a commit's patch.
    fs::write(dir.path().join("a.txt"), "dirty\n").unwrap();

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("diff --git a/a.txt b/a.txt"), "{patch}");
    assert!(patch.contains("-two\n+TWO\n"), "{patch}");
    assert!(patch.contains("diff --git a/b.txt b/b.txt"), "{patch}");
    assert!(patch.contains("new file mode"), "{patch}");
    assert!(!patch.contains("dirty"), "{patch}");
    let before = raw.rev("HEAD~1:a.txt");
    let after = raw.rev("HEAD:a.txt");
    assert!(
        patch.contains(&format!("index {before}..{after}")),
        "{patch}"
    );
}

#[test]
fn commit_patch_of_a_root_commit_adds_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "one\n")], "first", 100);
    let id = raw.rev("HEAD");

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("new file mode"), "{patch}");
    assert!(patch.contains("+one\n"), "{patch}");
}

#[test]
fn commit_patch_of_an_unknown_id_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "one\n")], "first", 100);

    let repo = Repo::discover_at(dir.path()).unwrap();

    assert!(
        repo.commit_patch("0123456789012345678901234567890123456789")
            .is_err()
    );
}

#[test]
fn tree_patch_shows_the_net_change_across_commits() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("keep.txt", "one\n")], "base", 100);
    let first = commit_id(&raw);
    commit_at(
        dir.path(),
        &raw,
        &[("keep.txt", "one\nflip\n"), ("new.txt", "draft\n")],
        "second",
        200,
    );
    commit_at(
        dir.path(),
        &raw,
        &[("keep.txt", "one\n"), ("new.txt", "final\n")],
        "third",
        300,
    );
    let tip = commit_id(&raw);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.tree_patch(&first, &tip).unwrap();

    assert!(!patch.contains("keep.txt"), "{patch}");
    assert!(patch.contains("new file mode"), "{patch}");
    assert!(patch.contains("+final\n"), "{patch}");
    assert!(!patch.contains("draft"), "{patch}");
}

#[test]
fn tree_patch_takes_any_revision() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "one\n")], "first", 100);
    commit_at(dir.path(), &raw, &[("a.txt", "two\n")], "second", 200);
    let tip = commit_id(&raw);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.tree_patch(&format!("{tip}^"), &tip).unwrap();

    assert!(patch.contains("-one\n+two\n"), "{patch}");
    assert!(repo.tree_patch("nope", &tip).is_err());
}

fn commit_id(repo: &Fixture) -> String {
    repo.rev("HEAD")
}

#[test]
fn commits_after_stop_at_the_base_even_past_a_merge() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "old", 100);
    commit_at(dir.path(), &raw, &[("a.txt", "b\n")], "base", 200);
    let base = commit_id(&raw);
    let main = raw.branch();
    checkout_new_branch(&raw, "feature");
    commit_at(dir.path(), &raw, &[("f.txt", "f\n")], "feature work", 300);
    // A merge whose second parent is an ancestor of the base.
    let (head, old) = (raw.rev("HEAD"), raw.rev("HEAD~2"));
    let args = [
        "commit-tree",
        "HEAD^{tree}",
        "-p",
        &head,
        "-p",
        &old,
        "-m",
        "merge old",
    ];
    let merge = as_ada(&raw, 400, &args, b"");
    raw.git(&["update-ref", "HEAD", &merge]);
    set_head(&raw, &main);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let summaries =
        |commits: Vec<Commit>| -> Vec<String> { commits.into_iter().map(|c| c.summary).collect() };

    assert_eq!(
        summaries(repo.commits_after(&base, "feature", 10).unwrap()),
        ["merge old", "feature work"]
    );
    assert!(repo.commits_after(&base, &base, 10).unwrap().is_empty());
    assert_eq!(
        repo.commits_after("0123456789012345678901234567890123456789", "feature", 10)
            .unwrap()
            .len(),
        4,
        "an unknown base hides nothing"
    );
}

/// `mktree` lines for `files`, each a blob at the root.
fn blob_entries(repo: &Fixture, files: &[(&str, &str)]) -> String {
    files
        .iter()
        .map(|(path, text)| {
            let blob = repo.git_with(&["hash-object", "-w", "--stdin"], text.as_bytes(), &[]);
            format!("100644 blob {blob}\t{path}\n")
        })
        .collect()
}

fn mktree(repo: &Fixture, listing: &str) -> String {
    repo.git_with(&["mktree"], listing.as_bytes(), &[])
}

/// A tree of `files`, each a path at the root and its text.
fn tree_of(repo: &Fixture, files: &[(&str, &str)]) -> String {
    mktree(repo, &blob_entries(repo, files))
}

/// The tree GitButler writes for a conflicted commit: `top` at the root,
/// `resolved` as `.auto-resolution`, one side, and the conflict list.
fn conflicted_tree(repo: &Fixture, top: &[(&str, &str)], resolved: &[(&str, &str)]) -> String {
    let auto = tree_of(repo, resolved);
    let side = tree_of(repo, &[("a.txt", "theirs\n")]);
    let mut listing = blob_entries(repo, top);
    listing.push_str(&format!("040000 tree {auto}\t.auto-resolution\n"));
    listing.push_str(&format!("040000 tree {side}\t.conflict-side-0\n"));
    let list = repo.git_with(
        &["hash-object", "-w", "--stdin"],
        b"ourEntries = [\"a.txt\"]\n",
        &[],
    );
    listing.push_str(&format!("100644 blob {list}\t.conflict-files\n"));
    mktree(repo, &listing)
}

/// Write a commit of `tree` on `parent`, with `header` (a `name value`
/// line) after the committer when given, and return its id.
fn commit_tree(
    repo: &Fixture,
    tree: String,
    parent: Option<&str>,
    message: &str,
    header: Option<&str>,
) -> String {
    let mut text = format!("tree {tree}\n");
    if let Some(parent) = parent {
        text.push_str(&format!("parent {parent}\n"));
    }
    text.push_str("author Ada Lovelace <ada@example.com> 100 +0000\n");
    text.push_str("committer Ada Lovelace <ada@example.com> 100 +0000\n");
    if let Some(header) = header {
        text.push_str(&format!("{header}\n"));
    }
    text.push('\n');
    text.push_str(message);
    let args = ["hash-object", "-t", "commit", "-w", "--stdin"];
    repo.git_with(&args, text.as_bytes(), &[])
}

const CONFLICTED: &str = "[conflict] change\n\nGitButler-Conflict: This is a GitButler-managed conflicted commit.\n   It keeps more trees.\n";

#[test]
fn commit_patch_of_a_gitbutler_conflicted_commit_shows_its_auto_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let base = commit_tree(
        &raw,
        tree_of(&raw, &[("a.txt", "one\n")]),
        None,
        "base",
        None,
    );
    let tree = conflicted_tree(&raw, &[("a.txt", "one\n")], &[("a.txt", "two\n")]);
    let id = commit_tree(&raw, tree, Some(base.as_str()), CONFLICTED, None);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("diff --git a/a.txt b/a.txt"), "{patch}");
    assert!(patch.contains("-one\n+two\n"), "{patch}");
    assert!(!patch.contains(".auto-resolution"), "{patch}");
    assert!(!patch.contains(".conflict-"), "{patch}");
}

#[test]
fn commit_patch_on_a_gitbutler_conflicted_parent_diffs_against_its_auto_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let tree = conflicted_tree(&raw, &[("a.txt", "one\n")], &[("a.txt", "two\n")]);
    let parent = commit_tree(&raw, tree, None, CONFLICTED, None);
    let files = [("a.txt", "two\n"), ("b.txt", "new\n")];
    let child = tree_of(&raw, &files);
    let id = commit_tree(&raw, child, Some(parent.as_str()), "child", None);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("diff --git a/b.txt b/b.txt"), "{patch}");
    assert!(!patch.contains("a.txt"), "{patch}");
    assert!(!patch.contains(".auto-resolution"), "{patch}");
    assert!(!patch.contains(".conflict-"), "{patch}");
}

#[test]
fn tree_patch_to_a_gitbutler_conflicted_tip_shows_its_auto_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let base = commit_tree(
        &raw,
        tree_of(&raw, &[("a.txt", "one\n")]),
        None,
        "base",
        None,
    );
    let tree = conflicted_tree(&raw, &[("a.txt", "one\n")], &[("a.txt", "two\n")]);
    let tip = commit_tree(&raw, tree, Some(base.as_str()), CONFLICTED, None);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.tree_patch(&format!("{tip}^"), &tip).unwrap();

    assert!(patch.contains("-one\n+two\n"), "{patch}");
    assert!(!patch.contains(".conflict-"), "{patch}");
}

#[test]
fn a_gitbutler_conflicted_header_marks_a_conflicted_commit() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let base = commit_tree(
        &raw,
        tree_of(&raw, &[("a.txt", "one\n")]),
        None,
        "base",
        None,
    );
    let tree = conflicted_tree(&raw, &[("a.txt", "one\n")], &[("a.txt", "two\n")]);
    let parent = Some(base.as_str());
    let id = commit_tree(&raw, tree, parent, "change", Some("gitbutler-conflicted 1"));

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("-one\n+two\n"), "{patch}");
    assert!(!patch.contains(".conflict-"), "{patch}");
}

#[test]
fn a_conflict_subject_alone_shows_the_tree_as_committed() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let base = commit_tree(
        &raw,
        tree_of(&raw, &[("a.txt", "one\n")]),
        None,
        "base",
        None,
    );
    let tree = conflicted_tree(&raw, &[("a.txt", "one\n")], &[("a.txt", "two\n")]);
    let parent = Some(base.as_str());
    let id = commit_tree(&raw, tree, parent, "[conflict] change", None);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let patch = repo.commit_patch(&id).unwrap();

    assert!(patch.contains("b/.auto-resolution/a.txt"), "{patch}");
    assert!(!patch.contains("-one\n+two\n"), "{patch}");
}

/// A repo whose `main` has `base` then, on `origin/main` only, `ahead`;
/// HEAD is `feature`, forked at `base` with one commit of its own.
fn forked(dir: &Path) -> (Fixture, String) {
    let raw = init_repo(dir);
    commit_at(dir, &raw, &[("a.txt", "a\n")], "base", 100);
    let base = commit_id(&raw);
    let main = raw.branch();
    commit_at(dir, &raw, &[("a.txt", "b\n")], "ahead", 200);
    raw.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    raw.git(&["update-ref", &format!("refs/heads/{main}"), &base]);
    checkout_new_branch(&raw, "feature");
    raw.git(&["reset", "-q", "--hard", &base]);
    commit_at(dir, &raw, &[("f.txt", "f\n")], "feature work", 300);
    (raw, base)
}

#[test]
fn branch_base_is_the_merge_base_with_origin_main_and_counts_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (_raw, base) = forked(dir.path());

    let repo = Repo::discover_at(dir.path()).unwrap();
    let found = repo.branch_base().unwrap().unwrap();

    assert_eq!(found.target, "origin/main");
    assert_eq!(found.commit.id, base);
    assert_eq!(found.behind, 1);
}

#[test]
fn branch_base_follows_origin_head_first() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, base) = forked(dir.path());
    raw.git(&["update-ref", "refs/remotes/origin/trunk", &base]);
    raw.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let found = repo.branch_base().unwrap().unwrap();

    assert_eq!(found.target, "origin/trunk");
    assert_eq!(found.behind, 0);
}

#[test]
fn branch_base_falls_back_to_a_local_main() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, base) = forked(dir.path());
    raw.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    raw.git(&["branch", "-m", &raw_main(&raw), "main"]);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let found = repo.branch_base().unwrap().unwrap();

    assert_eq!(found.target, "main");
    assert_eq!(found.commit.id, base);
}

#[test]
fn branch_base_falls_back_to_the_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "pushed", 100);
    let pushed = commit_id(&raw);
    raw.git(&["branch", "-m", &raw.branch(), "work"]);
    raw.git(&["update-ref", "refs/remotes/up/work", &pushed]);
    raw.git(&["config", "branch.work.remote", "up"]);
    raw.git(&["config", "branch.work.merge", "refs/heads/work"]);
    commit_at(dir.path(), &raw, &[("a.txt", "b\n")], "unpushed", 200);

    let repo = Repo::discover_at(dir.path()).unwrap();
    let found = repo.branch_base().unwrap().unwrap();

    assert_eq!(found.target, "up/work");
    assert_eq!(found.commit.id, pushed);
}

#[test]
fn branch_base_is_none_without_a_target_or_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    let raw = init_repo(dir.path());
    let repo = Repo::discover_at(dir.path()).unwrap();
    assert_eq!(repo.branch_base().unwrap(), None);

    commit_at(dir.path(), &raw, &[("a.txt", "a\n")], "only", 100);
    raw.git(&["branch", "-m", &raw.branch(), "work"]);
    let repo = Repo::discover_at(dir.path()).unwrap();
    assert_eq!(repo.branch_base().unwrap(), None);
}

/// The name `init_repo` gave the first branch.
fn raw_main(raw: &Fixture) -> String {
    let branches = raw.git(&["branch", "--format=%(refname:short)"]);
    branches
        .lines()
        .find(|name| *name != "feature")
        .unwrap()
        .to_string()
}
