use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{Opener, PrRef};
use crate::test_git::TestRepo;

/// An upstream repository standing in for GitHub, with pull request 7:
/// two commits on top of `main`'s first commit, while `main` moved on.
/// The pull request's head is reachable only from `refs/pull/7/head`.
struct Upstream {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: TestRepo,
    merge_base: String,
    head: String,
    commits: [String; 2],
}

impl Upstream {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("upstream");
        std::fs::create_dir(&path).unwrap();
        let repo = TestRepo::init(&path);
        repo.git(&["config", "uploadpack.allowFilter", "true"]);
        repo.git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
        std::fs::write(path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(path.join(".gitattributes"), "*.txt text\n").unwrap();
        repo.stage_all();
        let merge_base = repo.commit("base");
        let first = repo.commit_files_on("main", &[("a.txt", "one\n2\nthree\n")], "Edit a", 10);
        let second =
            repo.commit_files_on("main", &[("b.txt", "new\n")], "Add b\n\nWith a body.", 20);
        repo.git(&["update-ref", "refs/pull/7/head", &second]);
        repo.git(&["reset", "-q", "--hard", &merge_base]);
        std::fs::write(path.join("c.txt"), "later\n").unwrap();
        repo.stage_all();
        repo.commit("main moves on");
        Self {
            _dir: dir,
            root,
            repo,
            merge_base,
            head: second.clone(),
            commits: [first, second],
        }
    }

    fn url(&self) -> String {
        format!("file://{}", self.repo.dir().display())
    }

    /// A `gh` that answers pull request 7 as GitHub would, the compare
    /// in two pages, oldest commit first.
    fn gh(&self) -> OsString {
        self.gh_answering(true)
    }

    /// [`Self::gh`], failing every compare unless `answers_compare`.
    fn gh_answering(&self, answers_compare: bool) -> OsString {
        let pull = serde_json::json!({
            "title": "Make a better",
            "body": "Edits **a**.",
            "user": { "login": "ada" },
            "html_url": "https://github.com/acme/widgets/pull/7",
            "head": { "sha": self.head, "label": "ada:better-a" },
            "base": { "sha": self.repo.rev("main"), "ref": "main", "repo": { "clone_url": self.url() } },
        });
        let page = |sha: &str, message: &str, date: &str| {
            serde_json::json!({
                "merge_base_commit": { "sha": self.merge_base },
                "commits": [{
                    "sha": sha,
                    "commit": {
                        "message": message,
                        "author": { "name": "Ada Lovelace", "email": "ada@example.com", "date": date },
                    },
                }],
            })
        };
        let compare = serde_json::json!([
            page(&self.commits[0], "Edit a", "1970-01-01T00:00:10Z"),
            page(
                &self.commits[1],
                "Add b\n\nWith a body.",
                "1970-01-01T00:00:20Z"
            ),
        ]);
        std::fs::write(self.root.join("pull.json"), pull.to_string()).unwrap();
        std::fs::write(self.root.join("compare.json"), compare.to_string()).unwrap();
        let root = self.root.display();
        let compare = if answers_compare {
            format!("cat '{root}/compare.json'")
        } else {
            "echo 'HTTP 502' >&2; exit 1".to_string()
        };
        write_script(
            &self.root.join(if compare.starts_with("cat") {
                "gh"
            } else {
                "gh-no-compare"
            }),
            &format!(
                "case \"$*\" in\n\
                 'api --paginate --slurp repos/acme/widgets/compare/'*) {compare} ;;\n\
                 'api repos/acme/widgets/pulls/7') cat '{root}/pull.json' ;;\n\
                 *) echo \"unexpected gh $*\" >&2; exit 1 ;;\n\
                 esac\n"
            ),
        )
    }

    /// A clone of the upstream at `name` whose `origin` reads as the
    /// GitHub repository, with git sent to the upstream instead.
    fn clone(&self, name: &str, extra: &[&str]) -> TestRepo {
        let mut args = vec!["clone", "-q"];
        args.extend(extra);
        let url = self.url();
        args.extend([url.as_str(), name]);
        TestRepo::open(&self.root).git(&args);
        let clone = TestRepo::open(&self.root.join(name));
        let github = "https://github.com/acme/widgets.git";
        clone.git(&["remote", "set-url", "origin", github]);
        clone.git(&["config", &format!("url.{}.insteadOf", self.url()), github]);
        clone
    }

    fn opener(&self) -> Opener {
        Opener::new(self.gh(), self.root.join("cache"))
    }
}

fn write_script(path: &Path, body: &str) -> OsString {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.as_os_str().to_owned()
}

fn widgets() -> PrRef {
    "acme/widgets#7".parse().unwrap()
}

fn has_commit(repo: &TestRepo, id: &str) -> bool {
    std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{id}^{{commit}}")])
        .current_dir(repo.dir())
        .output()
        .unwrap()
        .status
        .success()
}

fn changed_paths(dir: &Path, base: &str, tip: &str) -> Vec<String> {
    let repo = deltoids::git::Repo::discover_at(dir).unwrap();
    let patch = repo.tree_patch(base, tip).unwrap();
    deltoids::parse::GitDiff::parse(&patch)
        .files
        .into_iter()
        .map(|file| file.new_path)
        .collect()
}

#[test]
fn opens_a_pull_request_in_the_clone_it_runs_in() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    assert!(!has_commit(&clone, &upstream.head));

    let opened = upstream
        .opener()
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();

    assert_eq!(opened.repo_dir, clone.dir());
    assert_eq!(
        opened.info,
        super::PullRequestInfo {
            number: 7,
            title: "Make a better".to_string(),
            author: "ada".to_string(),
            url: "https://github.com/acme/widgets/pull/7".to_string(),
            base: "main".to_string(),
            head: "ada:better-a".to_string(),
            body: "Edits **a**.".to_string(),
        }
    );
    assert_eq!(opened.merge_base, upstream.merge_base);
    assert_eq!(opened.head, upstream.head);
    let summaries: Vec<_> = opened.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["Add b", "Edit a"]);
    assert_eq!(opened.commits[0].message, "Add b\n\nWith a body.");
    assert_eq!(opened.commits[0].time, 20);
    assert!(has_commit(&clone, &upstream.head));
    assert_eq!(
        changed_paths(clone.dir(), &opened.merge_base, &opened.head),
        ["a.txt", "b.txt"]
    );
}

#[test]
fn leaves_the_clone_whole_and_its_refs_alone() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    let refs = clone.git(&["show-ref"]);

    upstream
        .opener()
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();

    assert_eq!(clone.git(&["show-ref"]), refs);
    assert_eq!(
        clone.git(&["rev-parse", "--is-shallow-repository"]),
        "false"
    );
    assert!(!clone.git_dir().join("FETCH_HEAD").exists());
    assert_eq!(clone.git(&["status", "--porcelain"]), "");
}

#[test]
fn reopening_needs_no_fetch() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    let opener = upstream.opener();
    opener.open(&widgets(), clone.dir(), &|_| {}).unwrap();
    std::fs::rename(upstream.repo.dir(), upstream.root.join("gone")).unwrap();

    let opened = opener.open(&widgets(), clone.dir(), &|_| {}).unwrap();

    assert_eq!(opened.head, upstream.head);
}

#[test]
fn opens_a_pull_request_in_a_partial_clone() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &["--filter=blob:none", "--no-checkout"]);

    let opened = upstream
        .opener()
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();

    assert_eq!(
        changed_paths(clone.dir(), &opened.merge_base, &opened.head),
        ["a.txt", "b.txt"]
    );
}

#[test]
fn without_a_clone_opens_a_shallow_cache_repository() {
    let upstream = Upstream::new();
    let elsewhere = upstream.root.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let opened = upstream
        .opener()
        .open(&widgets(), &elsewhere, &|_| {})
        .unwrap();

    let cache_dir = upstream
        .root
        .join("cache/deltoids/github/github.com/acme/widgets.git");
    assert_eq!(opened.repo_dir, cache_dir);
    let cache = TestRepo::open(&cache_dir);
    assert_eq!(cache.git(&["rev-parse", "--is-shallow-repository"]), "true");
    assert_eq!(cache.rev("HEAD"), upstream.head);
    assert_eq!(
        changed_paths(&cache_dir, &opened.merge_base, &opened.head),
        ["a.txt", "b.txt"]
    );
    let blob = upstream.repo.rev(&format!("{}:a.txt", upstream.head));
    let repo = deltoids::git::Repo::discover_at(&cache_dir).unwrap();
    assert_eq!(
        repo.blob_filtered(&blob, "a.txt").as_deref(),
        Some("one\n2\nthree\n")
    );
}

#[test]
fn a_clone_of_another_repository_is_not_used() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    clone.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/acme/gadgets.git",
    ]);

    let opened = upstream
        .opener()
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();

    assert_ne!(opened.repo_dir, clone.dir());
}

#[test]
fn reports_what_gh_says_when_it_fails() {
    let upstream = Upstream::new();
    let gh = write_script(
        &upstream.root.join("failing-gh"),
        "echo 'HTTP 404: Not Found' >&2; exit 1\n",
    );
    let opener = Opener::new(gh, upstream.root.join("cache"));

    let err = opener
        .open(&widgets(), &upstream.root, &|_| {})
        .unwrap_err();

    assert_eq!(
        err,
        "gh api repos/acme/widgets/pulls/7: HTTP 404: Not Found"
    );
}

#[test]
fn shows_the_pull_request_before_fetching_its_commits() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    let opener = upstream.opener();
    std::fs::rename(upstream.repo.dir(), upstream.root.join("gone")).unwrap();
    let shown = std::cell::RefCell::new(None);

    let result = opener.open(&widgets(), clone.dir(), &|info| {
        *shown.borrow_mut() = Some(info.title.clone());
    });

    assert!(result.is_err(), "the fetch needs the upstream");
    assert_eq!(shown.into_inner().as_deref(), Some("Make a better"));
}

#[test]
fn reopening_reads_the_comparison_kept_from_the_first_open() {
    let upstream = Upstream::new();
    let clone = upstream.clone("clone", &[]);
    upstream
        .opener()
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();
    let without_compare = Opener::new(upstream.gh_answering(false), upstream.root.join("cache"));

    let opened = without_compare
        .open(&widgets(), clone.dir(), &|_| {})
        .unwrap();

    assert_eq!(opened.merge_base, upstream.merge_base);
    assert_eq!(opened.commits.len(), 2);
}
