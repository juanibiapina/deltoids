//! Resolve the before/after text content for a [`FileDiff`].
//!
//! Available only when the `blob-resolve` cargo feature is enabled.
//!
//! `retrieve` looks up content via the git ODB (when a [`Repo`] is
//! provided), the working tree (when no hash or matching hash exists), and
//! finally reverse-applies the diff onto the resolved `after` to recover
//! `before` when only that side is missing.
//!
//! ODB blobs are read through git's *filtered* view
//! ([`Repo::blob_filtered`]), so files managed by a content filter
//! (git-crypt, transcrypt, any clean/smudge filter) resolve to their
//! working-tree-equivalent plaintext and render like any other file. On
//! non-filtered files this is byte-identical to the raw blob content.

use std::fs;

use crate::git::{Repo, blob_hash_matches, is_null_hash};
use crate::parse::FileDiff;
use crate::reverse;

/// Result of resolving one side (before/after) of a file's content.
pub enum SideContent {
    /// File is absent on this side (creation marker for `before`,
    /// deletion marker for `after`).
    Absent,
    /// Content was found in the git ODB or verified by hashing the
    /// filesystem against the expected blob hash.
    Resolved(String),
    /// A real (non-null) index hash was given but we cannot produce
    /// matching content. The hash is reported back so the caller can
    /// surface it to the user.
    Missing { hash: String },
}

/// Resolved content for both sides of a file diff.
pub struct FileContent {
    pub before: SideContent,
    pub after: SideContent,
}

/// Resolve `before` and `after` content for a file diff.
///
/// Both sides are resolved in one call so the ordering required to
/// reverse-reconstruct `before` from `after` stays an implementation
/// detail. For each side, resolution proceeds:
///
///   1. Null hash (`0000…`) → `Absent` (creation/deletion marker).
///   2. Read the blob from the repo's object database through git's
///      filtered view ([`Repo::blob_filtered`]). This resolves
///      committed, staged, and history blobs, applying any content
///      filter (git-crypt, transcrypt, custom clean/smudge) so filtered
///      files yield their plaintext.
///   3. Verify a candidate against the expected blob hash:
///        - `after`: the working-tree file at `file.new_path`.
///        - `before`: the diff reverse-applied onto the resolved
///          `after`.
///
///      Step 3 covers the common `git diff` working-tree case, where
///      the index hash is synthetic (not in the ODB) but the working
///      tree holds the matching content.
///   4. Otherwise report the hash as `Missing`.
///
/// When a side has no hash at all (non-git diff), the candidate is
/// returned without verification.
pub fn retrieve(file: &FileDiff, repo: Option<&Repo>) -> FileContent {
    let after = retrieve_after(file, repo);
    let after_text = match &after {
        SideContent::Resolved(s) => Some(s.as_str()),
        _ => None,
    };
    let before = retrieve_before(file, repo, after_text);
    FileContent { before, after }
}

fn retrieve_after(file: &FileDiff, repo: Option<&Repo>) -> SideContent {
    let Some(hash) = file.new_hash.as_deref() else {
        return match fs::read_to_string(&file.new_path) {
            Ok(content) => SideContent::Resolved(content),
            Err(_) => SideContent::Absent,
        };
    };

    if is_null_hash(hash) {
        return SideContent::Absent;
    }

    if let Some(repo) = repo
        && let Some(content) = repo.blob_filtered(hash, &file.new_path)
    {
        return SideContent::Resolved(content);
    }

    if let Ok(content) = fs::read_to_string(&file.new_path)
        && blob_hash_matches(&content, hash)
    {
        return SideContent::Resolved(content);
    }

    SideContent::Missing {
        hash: hash.to_string(),
    }
}

fn retrieve_before(file: &FileDiff, repo: Option<&Repo>, after: Option<&str>) -> SideContent {
    let Some(hash) = file.old_hash.as_deref() else {
        return match after {
            Some(after) => SideContent::Resolved(reverse::reconstruct_before(after, file)),
            None => SideContent::Absent,
        };
    };

    if is_null_hash(hash) {
        return SideContent::Absent;
    }

    if let Some(repo) = repo
        && let Some(content) = repo.blob_filtered(hash, &file.old_path)
    {
        return SideContent::Resolved(content);
    }

    if let Some(after) = after {
        let reconstructed = reverse::reconstruct_before(after, file);
        if blob_hash_matches(&reconstructed, hash) {
            return SideContent::Resolved(reconstructed);
        }
    }

    SideContent::Missing {
        hash: hash.to_string(),
    }
}
