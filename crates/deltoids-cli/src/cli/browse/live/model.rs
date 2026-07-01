//! The Live engine: an append-only feed of working-tree edits, built and
//! grown by one operation ([`LiveFeed::ingest`]).
//!
//! The feed answers "what is changing right now, from any source". Its
//! before-content comes from two places, unified by one code path:
//!
//! - **git supplies the baseline for free.** A first-seen file diffs its
//!   `HEAD` content (what [`build_model`] resolves as `before`) against
//!   its current working content. So a repo that is already dirty when
//!   Live opens emits one initial entry per dirty file, and a clean repo
//!   emits nothing.
//! - **`latest_state` supplies the delta.** After emitting an entry for a
//!   file, its current content is cached. The next change to that file
//!   diffs the cached last-known state against the new content, i.e. the
//!   specific delta since Live last saw it, not the cumulative diff vs
//!   `HEAD`.
//!
//! Because `latest_state` starts empty, startup and every later change
//! run the same `ingest`: no special seeding phase. Memory holds only
//! files that changed this session, bounded by activity, not repo size,
//! so there is never a full-repo scan.

use std::collections::HashMap;
use std::path::PathBuf;

use deltoids::{Diff, git};

use crate::cli::browse::files::model::{ResolvedFile, build_model};
use crate::sidebar::display_path;

/// One observed change to one file: the file's diff against its
/// last-known state at the moment the change was seen.
pub(super) struct FeedEntry {
    /// Workdir-relative path of the changed file.
    pub(super) path: String,
    /// Wall-clock time the entry was appended (`HH:MM:SS`).
    pub(super) timestamp: String,
    /// The diff from the file's last-known state to its new content.
    pub(super) diff: Diff,
}

/// The Live feed: the discovered repo, the per-file last-known content
/// cache, and the append-only feed itself.
pub(super) struct LiveFeed {
    /// The repo whose working tree is observed, if any. `None` outside a
    /// repo, in which case `ingest` is a no-op.
    repo: Option<git::Repo>,
    /// Last-seen content per touched file. Seeds each entry's "before":
    /// absent for a first-seen file (git's `HEAD` baseline is used
    /// instead), present for a repeat (the incremental delta).
    latest_state: HashMap<PathBuf, String>,
    /// The append-only feed, newest last.
    pub(super) entries: Vec<FeedEntry>,
}

impl LiveFeed {
    /// A feed with a repo. The initial `ingest` is the caller's job so
    /// build and reload share one path.
    pub(super) fn new(repo: Option<git::Repo>) -> Self {
        Self {
            repo,
            latest_state: HashMap::new(),
            entries: Vec::new(),
        }
    }

    /// An empty feed with no repo. Used as the startup placeholder and the
    /// degraded fallback.
    pub(super) fn empty() -> Self {
        Self::new(None)
    }

    /// Re-derive what changed from `working_tree_diff` + `latest_state`
    /// and append one entry per file whose current content differs from
    /// its last-known state. Returns whether any entry was appended.
    ///
    /// The same operation runs on build (empty `latest_state`, so dirty
    /// files emit HEAD->working entries) and on every reload (populated
    /// `latest_state`, so each entry is the incremental delta).
    pub(super) fn ingest(&mut self) -> Result<bool, String> {
        let Some(repo) = self.repo.as_ref() else {
            return Ok(false);
        };
        let input = repo.working_tree_diff()?;
        let model = build_model(&input, Some(repo))?;

        let mut appended = false;
        for resolved in model.files {
            appended |= self.ingest_file(resolved);
        }
        Ok(appended)
    }

    /// Append one entry for `resolved` if its current content differs
    /// from its last-known state; advance the cache. Returns whether an
    /// entry was appended.
    fn ingest_file(&mut self, resolved: ResolvedFile) -> bool {
        let path = display_path(&resolved.file).to_string();
        let key = PathBuf::from(&path);
        let before = self
            .latest_state
            .get(&key)
            .cloned()
            .unwrap_or(resolved.before);
        let after = resolved.after;
        if before == after {
            return false;
        }
        let diff = Diff::compute(&before, &after, &path);
        self.entries.push(FeedEntry {
            path,
            timestamp: now_hms(),
            diff,
        });
        self.latest_state.insert(key, after);
        true
    }

    /// The repo, for arming the working-tree watcher.
    pub(super) fn repo(&self) -> Option<&git::Repo> {
        self.repo.as_ref()
    }
}

/// Local wall-clock time as `HH:MM:SS`.
fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::live::test_support::*;
    use deltoids::LineKind;

    /// Sum added/removed lines across a diff's hunks.
    fn deltas(diff: &Diff) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for line in diff.hunks().iter().flat_map(|h| &h.lines) {
            match line.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => removed += 1,
                LineKind::Context => {}
            }
        }
        (added, removed)
    }

    fn feed_at(dir: &std::path::Path) -> LiveFeed {
        LiveFeed::new(git::Repo::discover_at(dir))
    }

    #[test]
    fn clean_repo_yields_empty_feed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let mut feed = feed_at(dir.path());
        assert!(!feed.ingest().unwrap(), "clean repo must append nothing");
        assert!(feed.entries.is_empty());
    }

    #[test]
    fn dirty_tracked_file_yields_head_to_working_entry() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Dirty the tree before opening the feed.
        std::fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        stage_all(&repo);

        let mut feed = feed_at(dir.path());
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);
        let entry = &feed.entries[0];
        assert_eq!(entry.path, "a.txt");
        // HEAD ("hello") -> working ("hello\nworld"): one added line.
        let (added, removed) = deltas(&entry.diff);
        assert_eq!((added, removed), (1, 0));
    }

    #[test]
    fn untracked_file_yields_entry_with_empty_before() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        std::fs::write(dir.path().join("new.txt"), "brand new\n").unwrap();
        stage_all(&repo);

        let mut feed = feed_at(dir.path());
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].path, "new.txt");
        // Empty -> "brand new": pure addition, nothing removed.
        let (added, removed) = deltas(&feed.entries[0].diff);
        assert_eq!((added, removed), (1, 0));
    }

    #[test]
    fn second_change_is_incremental_delta_not_cumulative() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "line1\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let mut feed = feed_at(dir.path());

        // First change: HEAD ("line1") -> "line1\nline2".
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\n").unwrap();
        stage_all(&repo);
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);

        // Second change: the delta is line2->line2\nline3, NOT the whole
        // diff vs HEAD (which would re-add line2).
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
        stage_all(&repo);
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 2);
        let (added, removed) = deltas(&feed.entries[1].diff);
        assert_eq!(
            (added, removed),
            (1, 0),
            "second entry must be the incremental delta (only line3 added)"
        );
    }

    #[test]
    fn first_edit_to_previously_clean_file_uses_head_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        // Repo starts clean; the feed sees no initial entries.
        let mut feed = feed_at(dir.path());
        assert!(!feed.ingest().unwrap());

        // Now edit: before must be HEAD content, so removing "two"
        // registers as one removal.
        std::fs::write(dir.path().join("a.txt"), "one\nthree\n").unwrap();
        stage_all(&repo);
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);
        let (added, removed) = deltas(&feed.entries[0].diff);
        assert_eq!((added, removed), (0, 1));
    }

    #[test]
    fn deleted_file_yields_full_deletion_entry() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "gone1\ngone2\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        std::fs::remove_file(dir.path().join("a.txt")).unwrap();

        let mut feed = feed_at(dir.path());
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].path, "a.txt");
        let (added, removed) = deltas(&feed.entries[0].diff);
        assert_eq!((added, removed), (0, 2), "deletion removes every line");
    }

    #[test]
    fn ingest_with_no_change_appends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        stage_all(&repo);
        commit_index(&repo, "init");

        let mut feed = feed_at(dir.path());
        std::fs::write(dir.path().join("a.txt"), "x\ny\n").unwrap();
        stage_all(&repo);
        assert!(feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);

        // Re-ingest with no on-disk change: latest_state already holds the
        // current content, so before == after and nothing is appended.
        assert!(!feed.ingest().unwrap());
        assert_eq!(feed.entries.len(), 1);
    }

    #[test]
    fn no_repo_ingest_is_noop() {
        let mut feed = LiveFeed::empty();
        assert!(!feed.ingest().unwrap());
        assert!(feed.entries.is_empty());
        assert!(feed.repo().is_none());
    }
}
