//! Lightweight wrappers over `git2` used by the [`content`](crate::content)
//! module to resolve before/after content for diff hunks.
//!
//! Available only when the `blob-resolve` cargo feature is enabled.

use git2::{ObjectType, Oid, Repository};

/// A discovered git repository, used to look up blobs by hash.
pub struct Repo(Repository);

impl Repo {
    /// Discover git repository from current directory.
    pub fn discover() -> Option<Self> {
        Repository::discover(".").ok().map(Repo)
    }

    /// Read a blob's text content by hash (abbreviated or full).
    /// Returns None if the hash is null, the blob is missing, or its
    /// bytes are not valid UTF-8.
    pub fn blob_text(&self, hash: &str) -> Option<String> {
        if is_null_hash(hash) {
            return None;
        }

        // For full 40-char hashes, parse directly; for abbreviated, use revparse
        let oid = if hash.len() == 40 {
            Oid::from_str(hash).ok()
        } else {
            self.0.revparse_single(hash).ok().map(|obj| obj.id())
        }?;

        let blob = self.0.find_blob(oid).ok()?;
        std::str::from_utf8(blob.content()).ok().map(String::from)
    }
}

/// Check if hash represents "no file" (all zeros).
pub fn is_null_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.chars().all(|c| c == '0')
}

/// Check whether `content` hashes to the git blob OID `expected`.
///
/// Accepts full (40-char) and abbreviated hashes. Returns `false` if
/// hashing fails or the hashes don't match.
pub fn blob_hash_matches(content: &str, expected: &str) -> bool {
    let Ok(oid) = Oid::hash_object(ObjectType::Blob, content.as_bytes()) else {
        return false;
    };
    let oid_str = oid.to_string();
    if expected.len() >= oid_str.len() {
        oid_str == *expected
    } else {
        oid_str.starts_with(expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_hash_detection() {
        assert!(is_null_hash("0000000"));
        assert!(is_null_hash("0000000000000000000000000000000000000000"));
        assert!(!is_null_hash("abc1234"));
        assert!(!is_null_hash("000000a"));
        assert!(!is_null_hash(""));
    }

    #[test]
    fn blob_hash_matches_full_and_abbreviated() {
        // git blob hash of "hello\n"
        let content = "hello\n";
        let full = "ce013625030ba8dba906f756967f9e9ca394464a";
        assert!(blob_hash_matches(content, full));
        assert!(blob_hash_matches(content, &full[..7]));
        assert!(blob_hash_matches(content, &full[..11]));
        assert!(!blob_hash_matches(content, "deadbeef"));
        assert!(!blob_hash_matches("other\n", full));
    }

    #[test]
    fn blob_lookup_abbreviated_hash() {
        // This test requires running in a git repo
        let repo = match Repo::discover() {
            Some(r) => r,
            None => return, // Skip if not in a git repo
        };

        // Get HEAD commit's tree to find a known blob (file, not directory)
        let head = repo.0.head().unwrap().peel_to_commit().unwrap();
        let tree = head.tree().unwrap();
        let entry = tree
            .iter()
            .find(|e| e.kind() == Some(git2::ObjectType::Blob))
            .expect("should have at least one blob in tree");
        let full_hash = entry.id().to_string();
        let abbrev_hash = &full_hash[..7];

        // Both should resolve to the same content
        let full_content = repo.blob_text(&full_hash);
        let abbrev_content = repo.blob_text(abbrev_hash);

        assert!(full_content.is_some(), "full hash should resolve");
        assert!(abbrev_content.is_some(), "abbreviated hash should resolve");
        assert_eq!(
            full_content, abbrev_content,
            "both should return same content"
        );
    }
}
