//! Parse unified diff format into structured data.

use regex::Regex;
use std::sync::OnceLock;

/// A parsed git diff containing multiple file diffs.
#[derive(Debug, Clone)]
pub struct GitDiff {
    pub files: Vec<FileDiff>,
}

/// A diff for a single file.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    /// Original path if this file was renamed.
    pub rename_from: Option<String>,
    /// Hash of the old blob (from git index line).
    pub old_hash: Option<String>,
    /// Hash of the new blob (from git index line).
    pub new_hash: Option<String>,
    pub hunks: Vec<RawHunk>,
}

/// A raw hunk from the diff (before enrichment).
#[derive(Debug, Clone)]
pub struct RawHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<RawLine>,
}

/// A single line from a raw hunk.
#[derive(Debug, Clone)]
pub struct RawLine {
    pub kind: RawLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawLineKind {
    Context,
    Added,
    Removed,
}

static HUNK_HEADER_RE: OnceLock<Regex> = OnceLock::new();
static INDEX_RE: OnceLock<Regex> = OnceLock::new();

fn hunk_header_re() -> &'static Regex {
    HUNK_HEADER_RE
        .get_or_init(|| Regex::new(r"^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@").unwrap())
}

fn index_re() -> &'static Regex {
    INDEX_RE.get_or_init(|| Regex::new(r"^index ([0-9a-f]+)\.\.([0-9a-f]+)").unwrap())
}

impl GitDiff {
    /// Parse a unified diff string into structured data.
    pub fn parse(diff: &str) -> Self {
        let mut files = Vec::new();
        let mut current_file: Option<FileDiff> = None;
        let mut current_hunk: Option<RawHunk> = None;
        let mut old_path = String::new();
        let mut rename_from: Option<String> = None;
        let mut pending_old_hash: Option<String> = None;
        let mut pending_new_hash: Option<String> = None;

        for line in diff.lines() {
            // Parse index line for blob hashes
            if let Some(caps) = index_re().captures(line) {
                pending_old_hash = Some(caps.get(1).unwrap().as_str().to_string());
                pending_new_hash = Some(caps.get(2).unwrap().as_str().to_string());
                continue;
            }
            if let Some(path) = line.strip_prefix("rename from ") {
                rename_from = Some(path.to_string());
                continue;
            }
            if line.starts_with("rename to ") {
                // rename_from already captured, will be used when file is created
                continue;
            }
            if let Some(path) = line.strip_prefix("--- ") {
                // Finish previous file if any
                if let Some(mut file) = current_file.take() {
                    if let Some(hunk) = current_hunk.take() {
                        file.hunks.push(hunk);
                    }
                    files.push(file);
                }
                old_path = strip_prefix_ab(path);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                let new_path = strip_prefix_ab(path);
                current_file = Some(FileDiff {
                    old_path: old_path.clone(),
                    new_path,
                    rename_from: rename_from.take(),
                    old_hash: pending_old_hash.take(),
                    new_hash: pending_new_hash.take(),
                    hunks: Vec::new(),
                });
            } else if let Some(caps) = hunk_header_re().captures(line) {
                // Finish previous hunk if any
                if let Some(hunk) = current_hunk.take()
                    && let Some(ref mut file) = current_file
                {
                    file.hunks.push(hunk);
                }

                let old_start: usize = caps.get(1).unwrap().as_str().parse().unwrap_or(1);
                let old_count: usize = caps
                    .get(2)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let new_start: usize = caps.get(3).unwrap().as_str().parse().unwrap_or(1);
                let new_count: usize = caps
                    .get(4)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);

                current_hunk = Some(RawHunk {
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    lines: Vec::new(),
                });
            } else if current_hunk.is_some() {
                let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
                    (RawLineKind::Added, rest.to_string())
                } else if let Some(rest) = line.strip_prefix('-') {
                    (RawLineKind::Removed, rest.to_string())
                } else if let Some(rest) = line.strip_prefix(' ') {
                    (RawLineKind::Context, rest.to_string())
                } else if line.is_empty() {
                    // Empty context line (no leading space in some diffs)
                    (RawLineKind::Context, String::new())
                } else {
                    // Skip non-diff lines (e.g., "\ No newline at end of file")
                    continue;
                };

                if let Some(ref mut hunk) = current_hunk {
                    hunk.lines.push(RawLine { kind, content });
                }
            }
        }

        // Finish last hunk and file
        if let Some(hunk) = current_hunk
            && let Some(ref mut file) = current_file
        {
            file.hunks.push(hunk);
        }
        if let Some(file) = current_file {
            files.push(file);
        }

        GitDiff { files }
    }
}

/// Strip "a/" or "b/" prefix from git diff paths.
fn strip_prefix_ab(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_diff() {
        let diff = r#"--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
     let x = 1;
 }
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].new_path, "src/main.rs");
        assert_eq!(parsed.files[0].hunks.len(), 1);
        assert_eq!(parsed.files[0].hunks[0].lines.len(), 4);
    }

    #[test]
    fn parse_multiple_hunks() {
        let diff = r#"--- a/lib.rs
+++ b/lib.rs
@@ -1,2 +1,2 @@
-old line 1
+new line 1
 unchanged
@@ -10,2 +10,3 @@
 context
+added
 more context
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].hunks.len(), 2);
    }

    #[test]
    fn parse_multiple_files() {
        let diff = r#"--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old
+new
--- a/file2.rs
+++ b/file2.rs
@@ -1 +1 @@
-foo
+bar
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[0].new_path, "file1.rs");
        assert_eq!(parsed.files[1].new_path, "file2.rs");
    }

    #[test]
    fn strips_ab_prefix() {
        assert_eq!(strip_prefix_ab("a/src/main.rs"), "src/main.rs");
        assert_eq!(strip_prefix_ab("b/src/main.rs"), "src/main.rs");
        assert_eq!(strip_prefix_ab("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn parse_rename() {
        let diff = r#"diff --git a/old_name.rs b/new_name.rs
similarity index 100%
rename from old_name.rs
rename to new_name.rs
--- a/old_name.rs
+++ b/new_name.rs
@@ -1,2 +1,2 @@
 fn main() {
-    old();
+    new();
 }
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].rename_from, Some("old_name.rs".to_string()));
        assert_eq!(parsed.files[0].new_path, "new_name.rs");
    }

    #[test]
    fn parse_index_with_mode() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old
+new
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].old_hash, Some("abc1234".to_string()));
        assert_eq!(parsed.files[0].new_hash, Some("def5678".to_string()));
    }

    #[test]
    fn parse_index_without_mode() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old
+new
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].old_hash, Some("abc1234".to_string()));
        assert_eq!(parsed.files[0].new_hash, Some("def5678".to_string()));
    }

    #[test]
    fn parse_no_index_line() {
        let diff = r#"--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old
+new
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].old_hash, None);
        assert_eq!(parsed.files[0].new_hash, None);
    }
}
