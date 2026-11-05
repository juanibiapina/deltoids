//! Read-only history: local branches, the commits reachable from one, and
//! the patch a commit introduces.

use gix::ObjectId;
use gix::bstr::ByteSlice;

use super::diff::{self, Overlay};
use super::{Error, Repo, patch};

/// One local branch, as a branch list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    /// Whether this branch is checked out.
    pub head: bool,
    /// Subject line of the branch's tip commit.
    pub summary: String,
    /// Committer time of the tip commit, in seconds since the epoch.
    pub time: i64,
}

/// One commit, as a commit list and its detail view show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full hex object id.
    pub id: String,
    /// The first seven hex digits of `id`.
    pub short_id: String,
    pub summary: String,
    /// The full message, subject and body.
    pub message: String,
    pub author: String,
    pub email: String,
    /// Author time, in seconds since the epoch.
    pub time: i64,
}

/// Where the checked-out branch forks from the branch it will merge
/// into, as a stack view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchBase {
    /// The ref the base came from, shortened: `origin/main`.
    pub target: String,
    /// The merge base of `HEAD` and `target`.
    pub commit: Commit,
    /// Commits `target` has past the base.
    pub behind: usize,
}

/// How many commits [`Repo::branch_base`] counts behind before it stops.
const BEHIND_CAP: usize = 10_000;

impl Repo {
    /// The base of the checked-out branch: its merge base with the first
    /// target that resolves, of `origin/HEAD`, `origin/main`,
    /// `origin/master`, a local `main` or `master` other than the
    /// checked-out branch, and the branch's upstream. `None` when `HEAD`
    /// is unborn or no target shares history with it.
    pub fn branch_base(&self) -> Result<Option<BranchBase>, String> {
        self.read_branch_base()
            .map_err(|e| format!("failed to find the branch base: {e}"))
    }

    fn read_branch_base(&self) -> Result<Option<BranchBase>, Error> {
        let Ok(head) = self.repo.head_id() else {
            return Ok(None);
        };
        let head = head.detach();
        for target in self.base_targets() {
            let Ok(mut reference) = self.repo.find_reference(target.as_str()) else {
                continue;
            };
            let Ok(tip) = reference.peel_to_commit() else {
                continue;
            };
            let tip = tip.id;
            let Some(base) = self.repo.merge_base(head, tip).ok().flatten() else {
                continue;
            };
            let base = base.detach();
            let behind = self
                .repo
                .rev_walk([tip])
                .with_hidden([base])
                .all()?
                .take(BEHIND_CAP)
                .count();
            return Ok(Some(BranchBase {
                target: shorten(&target),
                commit: commit_of(&self.repo.find_commit(base)?)?,
                behind,
            }));
        }
        Ok(None)
    }

    /// Full ref names to try as the base's target, best first.
    fn base_targets(&self) -> Vec<String> {
        let head = self
            .repo
            .head_name()
            .ok()
            .flatten()
            .map(|name| name.as_bstr().to_str_lossy().into_owned());
        let mut targets = Vec::new();
        if let Ok(origin_head) = self.repo.find_reference("refs/remotes/origin/HEAD")
            && let Some(name) = origin_head.target().try_name()
        {
            targets.push(name.as_bstr().to_str_lossy().into_owned());
        }
        targets.push("refs/remotes/origin/main".into());
        targets.push("refs/remotes/origin/master".into());
        for local in ["refs/heads/main", "refs/heads/master"] {
            if head.as_deref() != Some(local) {
                targets.push(local.into());
            }
        }
        if let Some(upstream) = head.as_deref().and_then(|head| self.upstream_of(head)) {
            targets.push(upstream);
        }
        targets
    }

    /// The tracking ref of local branch `head` (`refs/heads/<name>`),
    /// from its `branch.<name>.remote` and `.merge` config.
    fn upstream_of(&self, head: &str) -> Option<String> {
        let name = head.strip_prefix("refs/heads/")?;
        let remote = self.local_config_string(&format!("branch.{name}.remote"))?;
        let merge = self.local_config_string(&format!("branch.{name}.merge"))?;
        let branch = merge.strip_prefix("refs/heads/")?;
        Some(if remote == "." {
            format!("refs/heads/{branch}")
        } else {
            format!("refs/remotes/{remote}/{branch}")
        })
    }

    /// Every local branch: the checked-out one first, the rest by the
    /// time of their tip commit, newest first. Empty before the first
    /// commit. A branch whose ref or tip cannot be read is left out.
    pub fn local_branches(&self) -> Result<Vec<Branch>, String> {
        self.read_branches()
            .map_err(|e| format!("failed to list branches: {e}"))
    }

    fn read_branches(&self) -> Result<Vec<Branch>, Error> {
        let head = self.repo.head_name().ok().flatten();
        let mut branches = Vec::new();
        for mut reference in self.repo.references()?.local_branches()?.flatten() {
            let Ok(name) = reference.name().shorten().to_str().map(str::to_string) else {
                continue;
            };
            let is_head = head
                .as_ref()
                .is_some_and(|h| h.as_ref() == reference.name());
            let Ok(tip) = reference.peel_to_commit() else {
                continue;
            };
            branches.push(Branch {
                name,
                head: is_head,
                summary: tip.message()?.summary().to_str_lossy().into_owned(),
                time: tip.time()?.seconds,
            });
        }
        branches.sort_by(|a, b| {
            b.head
                .cmp(&a.head)
                .then(b.time.cmp(&a.time))
                .then(a.name.cmp(&b.name))
        });
        Ok(branches)
    }

    /// Up to `limit` commits reachable from `rev` (a branch name or any
    /// revision), newest commit date first, as `git log` lists them. Reads
    /// only the commits it returns and their parents, so the cost follows
    /// `limit`, whatever the size of the history.
    pub fn commits(&self, rev: &str, limit: usize) -> Result<Vec<Commit>, String> {
        self.walk(rev, None, limit)
            .map_err(|e| format!("failed to read commits of {rev}: {e}"))
    }

    /// [`Self::commits`] stopping at `base`: the commits reachable from
    /// `rev` but not from `base`, as `git log base..rev` lists them. A
    /// `base` that does not resolve hides nothing.
    pub fn commits_after(
        &self,
        base: &str,
        rev: &str,
        limit: usize,
    ) -> Result<Vec<Commit>, String> {
        let base = self.commit_id(base).ok();
        self.walk(rev, base, limit)
            .map_err(|e| format!("failed to read commits of {rev}: {e}"))
    }

    fn commit_id(&self, rev: &str) -> Result<ObjectId, Error> {
        let object = self.repo.rev_parse_single(rev)?.object()?;
        Ok(object.peel_to_commit()?.id)
    }

    fn walk(&self, rev: &str, base: Option<ObjectId>, limit: usize) -> Result<Vec<Commit>, Error> {
        let start = self.commit_id(rev)?;
        let walk = self
            .repo
            .rev_walk([start])
            .with_hidden(base)
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                Default::default(),
            ))
            .all()?;
        let mut commits = Vec::new();
        for info in walk.take(limit) {
            commits.push(commit_of(&info?.object()?)?);
        }
        Ok(commits)
    }

    /// The patch commit `id` introduces against its first parent (every
    /// file added for a root commit), with full blob hashes so
    /// [`crate::content::retrieve`] resolves both sides from the object
    /// database. A GitButler conflicted commit, on either side, is
    /// diffed by its auto-resolved tree, as GitButler shows it.
    pub fn commit_patch(&self, id: &str) -> Result<String, String> {
        let read = || -> Result<String, Error> {
            let commit = self.repo.find_commit(ObjectId::from_hex(id.as_bytes())?)?;
            let tree = shown_tree(&self.repo, &commit)?;
            let parent = match commit.parent_ids().next() {
                Some(parent) => Some(shown_tree(&self.repo, &self.repo.find_commit(parent)?)?),
                None => None,
            };
            print_patch(&self.repo, parent, tree)
        };
        read().map_err(|e| format!("failed to diff commit {id}: {e}"))
    }

    /// The patch from revision `base`'s tree to revision `tip`'s: the
    /// net change of every commit between them, in the form
    /// [`Self::commit_patch`] gives, GitButler conflicted commits
    /// included.
    pub fn tree_patch(&self, base: &str, tip: &str) -> Result<String, String> {
        let tree = |rev: &str| -> Result<ObjectId, Error> {
            let object = self.repo.rev_parse_single(rev)?.object()?;
            match object.kind {
                gix::object::Kind::Commit => shown_tree(&self.repo, &object.into_commit()),
                _ => Ok(object.peel_to_tree()?.id),
            }
        };
        let read = || print_patch(&self.repo, Some(tree(base)?), tree(tip)?);
        read().map_err(|e| format!("failed to diff {base}..{tip}: {e}"))
    }
}

/// `origin/main` for `refs/remotes/origin/main`, `main` for
/// `refs/heads/main`.
fn shorten(full: &str) -> String {
    full.strip_prefix("refs/remotes/")
        .or_else(|| full.strip_prefix("refs/heads/"))
        .unwrap_or(full)
        .to_string()
}

fn commit_of(commit: &gix::Commit<'_>) -> Result<Commit, Error> {
    let id = commit.id.to_string();
    let author = commit.author()?;
    Ok(Commit {
        short_id: id[..7].to_string(),
        id,
        summary: commit.message()?.summary().to_str_lossy().into_owned(),
        message: commit.message_raw()?.to_str_lossy().into_owned(),
        author: author.name.to_str_lossy().into_owned(),
        email: author.email.to_str_lossy().into_owned(),
        time: author.time()?.seconds,
    })
}

/// The tree `commit` shows: its `.auto-resolution` subtree when
/// GitButler wrote it as a conflicted commit, else its own tree.
///
/// GitButler's rule (`Commit::is_conflicted` in `but-core`): the commit
/// carries conflict metadata, a `GitButler-Conflict` message trailer or a
/// numeric `gitbutler-conflicted` header, and its tree holds synthetic
/// entries next to the files. Those entries are whole copies of the
/// repository, so diffing the raw tree would list every file several
/// times over. Without an `.auto-resolution` entry the raw tree is shown.
fn shown_tree(repo: &gix::Repository, commit: &gix::Commit<'_>) -> Result<ObjectId, Error> {
    let tree = commit.tree_id()?.detach();
    if !has_conflict_metadata(commit) {
        return Ok(tree);
    }
    let resolved = repo
        .find_tree(tree)?
        .find_entry(".auto-resolution")
        .map(|entry| entry.object_id());
    Ok(resolved.unwrap_or(tree))
}

fn has_conflict_metadata(commit: &gix::Commit<'_>) -> bool {
    let Ok(decoded) = commit.decode() else {
        return false;
    };
    let trailer =
        trailers(decoded.message).any(|(key, _)| key.eq_ignore_ascii_case(b"GitButler-Conflict"));
    let header = decoded
        .extra_headers()
        .find("gitbutler-conflicted")
        .and_then(|value| value.to_str().ok()?.trim().parse::<u64>().ok())
        .is_some();
    trailer || header
}

/// `Key: value` lines of a message's last paragraph, as git trailers.
/// A message without a body has none.
fn trailers(message: &gix::bstr::BStr) -> impl Iterator<Item = (&[u8], &[u8])> {
    let text = message.trim_end();
    let paragraph = text.rfind(b"\n\n").map_or(&[][..], |i| &text[i + 2..]);
    paragraph.lines().filter_map(|line| {
        let colon = line.find_byte(b':')?;
        let key = line[..colon].trim();
        let valid = !key.is_empty()
            && key
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_');
        valid.then(|| (key, line[colon + 1..].trim()))
    })
}

fn print_patch(
    repo: &gix::Repository,
    old: Option<ObjectId>,
    new: ObjectId,
) -> Result<String, Error> {
    let overlay = Overlay::new();
    let changes = diff::tree_changes(repo, old, new)?;
    patch::write(&changes, |side| diff::content(repo, &overlay, side))
}
