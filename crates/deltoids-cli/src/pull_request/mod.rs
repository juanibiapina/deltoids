//! Open a GitHub pull request for review.
//!
//! [`Opener::open`] asks GitHub, through the `gh` CLI, for the pull
//! request's head and the merge base its diff starts from, as GitHub's
//! "Files changed" tab shows it. Then it makes sure both commits are in a
//! local repository that the diff engine can read:
//!
//! - A clone of the pull request's repository that contains the working
//!   directory: a remote of it names `owner/repo`. Missing commits are
//!   fetched by id, without touching refs, `FETCH_HEAD`, or the working
//!   tree.
//! - Otherwise a bare cache repository per GitHub repository, fetched
//!   shallow and without blobs. Its `HEAD` points at the pull request's
//!   head, which the engine reads attributes from.
//!
//! The engine reads objects through `gix`, which cannot fetch missing
//! blobs from a partial repository. In one, the git CLI first fetches the
//! blobs the diff reads.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use deltoids::git;

mod github;
mod local;
mod pr_ref;
#[cfg(test)]
mod tests;

pub(crate) use pr_ref::PrRef;

/// What a reviewer reads about a pull request before its diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequestInfo {
    pub(crate) number: u64,
    pub(crate) title: String,
    /// The author's login.
    pub(crate) author: String,
    /// The pull request's page on GitHub.
    pub(crate) url: String,
    /// The branch it merges into.
    pub(crate) base: String,
    /// The branch it comes from, as `owner:branch`.
    pub(crate) head: String,
    /// The description, in Markdown; empty when there is none.
    pub(crate) body: String,
}

/// A pull request whose merge base and head are in a local repository.
#[derive(Debug)]
pub(crate) struct OpenedPr {
    /// The clone, or the cache repository.
    pub(crate) repo_dir: PathBuf,
    pub(crate) info: PullRequestInfo,
    /// Full id of the commit the diff starts from.
    pub(crate) merge_base: String,
    /// Full id of the pull request's head commit.
    pub(crate) head: String,
    /// The pull request's commits, newest first.
    pub(crate) commits: Vec<git::Commit>,
}

/// Opens pull requests with one `gh` program and one cache directory.
pub(crate) struct Opener {
    gh: OsString,
    /// Where cache repositories live; `None` when no cache directory is
    /// known.
    cache: Option<PathBuf>,
}

impl Opener {
    /// `gh` from `PATH`, and the cache under `$XDG_CACHE_HOME` (or
    /// `~/.cache`).
    pub(crate) fn from_env() -> Self {
        Self {
            gh: OsString::from("gh"),
            cache: cache_root(),
        }
    }

    #[cfg(test)]
    fn new(gh: OsString, cache: PathBuf) -> Self {
        Self {
            gh,
            cache: Some(cache),
        }
    }

    /// Resolve `pr` with GitHub, find or make a local repository holding
    /// its merge base and head, and fetch what it lacks. `cwd` selects
    /// the clone. Blocks on the network.
    ///
    /// `shown` gets what a reviewer reads first as soon as GitHub answers
    /// with it, before any commit is fetched. Work that does not depend
    /// on other work runs at the same time: GitHub compares the commits
    /// while the head is fetched.
    pub(crate) fn open(
        &self,
        pr: &PrRef,
        cwd: &Path,
        shown: &dyn Fn(&PullRequestInfo),
    ) -> Result<OpenedPr, String> {
        let pull = github::pull(&self.gh, pr)?;
        shown(&pull.info);
        let local = match local::Local::clone_of(pr, cwd) {
            Some(clone) => clone,
            None => {
                let root = self
                    .cache
                    .as_ref()
                    .ok_or("No clone of the repository here, and no cache directory")?;
                local::Local::cache(root, pr, &pull.clone_url, &self.gh)?
            }
        };
        let (compared, fetched) = std::thread::scope(|scope| {
            let compared = scope.spawn(|| self.compare(pr, &pull.base, &pull.head));
            let fetched = if local.is_cache() {
                local.fetch_commits(&[&pull.head, &pull.base])
            } else {
                local.fetch_commits(&[&pull.head])
            };
            let compared = compared
                .join()
                .unwrap_or_else(|_| Err("comparing the commits failed".to_string()));
            (compared, fetched)
        });
        let compared = compared?;
        fetched?;
        local.fetch_commits(&[&pull.head, &compared.merge_base])?;
        local.point_head(&pull.head)?;
        local.fetch_diff_blobs(&compared.merge_base, &pull.head)?;
        Ok(OpenedPr {
            repo_dir: local.dir,
            info: pull.info,
            merge_base: compared.merge_base,
            head: pull.head,
            commits: compared.commits,
        })
    }

    /// GitHub's comparison of `base...head`. It never changes for the
    /// same two commits, so it is kept in the cache directory and read
    /// from there when the pull request opens again.
    fn compare(&self, pr: &PrRef, base: &str, head: &str) -> Result<github::Compared, String> {
        let kept = self.cache.as_ref().map(|root| {
            root.join("deltoids")
                .join("github")
                .join(&pr.host)
                .join(&pr.owner)
                .join(format!("{}.compare", pr.repo))
                .join(format!("{base}...{head}.json"))
        });
        if let Some(kept) = &kept
            && let Ok(pages) = std::fs::read(kept)
            && let Ok(compared) = github::compare(&pages, pr, base, head)
        {
            return Ok(compared);
        }
        let pages = github::compare_pages(&self.gh, pr, base, head)?;
        if let Some(kept) = &kept
            && let Some(dir) = kept.parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let _ = std::fs::write(kept, &pages);
        }
        github::compare(&pages, pr, base, head)
    }
}

/// `$XDG_CACHE_HOME`, or `~/.cache` when it is unset or relative.
fn cache_root() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    xdg.or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
}
