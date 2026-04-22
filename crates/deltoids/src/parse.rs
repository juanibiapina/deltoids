//! Parse unified diff format into structured data.

use regex::Regex;
use std::sync::OnceLock;

/// A parsed git diff containing multiple file diffs.
#[derive(Debug, Clone)]
pub struct GitDiff {
    pub files: Vec<FileDiff>,
    /// Non-diff lines after all file diffs (trailing commit metadata, etc.).
    /// Also captures entire input when no diff content is present.
    pub trailing_preamble: Option<Vec<String>>,
}

/// A diff for a single file.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Non-diff lines preceding this file (commit metadata, etc.).
    pub preamble: Vec<String>,
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

#[derive(Debug, Clone)]
enum HunkBodyEvent {
    Raw(RawLine),
    Skip,
    Preamble(String),
}

static HUNK_HEADER_RE: OnceLock<Regex> = OnceLock::new();
static INDEX_RE: OnceLock<Regex> = OnceLock::new();
static ANSI_RE: OnceLock<Regex> = OnceLock::new();

fn hunk_header_re() -> &'static Regex {
    HUNK_HEADER_RE
        .get_or_init(|| Regex::new(r"^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@").unwrap())
}

fn index_re() -> &'static Regex {
    INDEX_RE.get_or_init(|| Regex::new(r"^index ([0-9a-f]+)\.\.([0-9a-f]+)").unwrap())
}

fn ansi_re() -> &'static Regex {
    ANSI_RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap())
}

fn strip_ansi(s: &str) -> String {
    ansi_re().replace_all(s, "").to_string()
}

fn parse_hunk_body_line(line: &str) -> HunkBodyEvent {
    if let Some(rest) = line.strip_prefix('+') {
        return HunkBodyEvent::Raw(RawLine {
            kind: RawLineKind::Added,
            content: rest.to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix('-') {
        return HunkBodyEvent::Raw(RawLine {
            kind: RawLineKind::Removed,
            content: rest.to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix(' ') {
        return HunkBodyEvent::Raw(RawLine {
            kind: RawLineKind::Context,
            content: rest.to_string(),
        });
    }
    if line.is_empty() {
        return HunkBodyEvent::Raw(RawLine {
            kind: RawLineKind::Context,
            content: String::new(),
        });
    }
    if line.starts_with("\\ ") {
        return HunkBodyEvent::Skip;
    }
    HunkBodyEvent::Preamble(line.to_string())
}

struct ParseState {
    files: Vec<FileDiff>,
    current_file: Option<FileDiff>,
    current_hunk: Option<RawHunk>,
    old_path: String,
    rename_from: Option<String>,
    pending_rename_to: Option<String>,
    pending_old_hash: Option<String>,
    pending_new_hash: Option<String>,
    pending_preamble: Vec<String>,
}

impl ParseState {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            current_file: None,
            current_hunk: None,
            old_path: String::new(),
            rename_from: None,
            pending_rename_to: None,
            pending_old_hash: None,
            pending_new_hash: None,
            pending_preamble: Vec::new(),
        }
    }

    fn finish_hunk(&mut self) {
        if let Some(hunk) = self.current_hunk.take()
            && let Some(ref mut file) = self.current_file
        {
            file.hunks.push(hunk);
        }
    }

    fn finish_file(&mut self) {
        self.finish_hunk();
        if let Some(file) = self.current_file.take() {
            self.files.push(file);
        }
    }

    /// Create FileDiff for pure renames (100% similarity, no content changes).
    /// These have no --- / +++ lines, so we must create the file from rename info.
    fn finish_pending_rename(&mut self) {
        if let (Some(old_path), Some(new_path)) =
            (self.rename_from.take(), self.pending_rename_to.take())
        {
            // Only create if we don't already have a current_file
            // (renames with content changes will have --- / +++ lines)
            if self.current_file.is_none() {
                self.files.push(FileDiff {
                    preamble: std::mem::take(&mut self.pending_preamble),
                    old_path: old_path.clone(),
                    new_path,
                    rename_from: Some(old_path),
                    old_hash: self.pending_old_hash.take(),
                    new_hash: self.pending_new_hash.take(),
                    hunks: Vec::new(),
                });
            }
        }
    }

    fn push_raw_line(&mut self, raw_line: RawLine) {
        if let Some(ref mut hunk) = self.current_hunk {
            hunk.lines.push(raw_line);
        }
    }

    fn collect_preamble_after_hunk(&mut self, preamble: String) {
        self.finish_hunk();
        self.pending_preamble.push(preamble);
    }

    fn apply_hunk_body_event(&mut self, event: HunkBodyEvent) -> bool {
        match event {
            HunkBodyEvent::Raw(raw_line) => {
                self.push_raw_line(raw_line);
                false
            }
            HunkBodyEvent::Skip => true,
            HunkBodyEvent::Preamble(preamble) => {
                self.collect_preamble_after_hunk(preamble);
                true
            }
        }
    }

    fn handle_in_hunk_line(&mut self, line: &str) -> bool {
        self.apply_hunk_body_event(parse_hunk_body_line(line))
    }

    fn into_diff(mut self) -> GitDiff {
        self.finish_pending_rename();
        self.finish_file();

        // Capture any remaining preamble (non-diff content at end, or entire non-diff input)
        let trailing_preamble = if self.pending_preamble.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.pending_preamble))
        };

        GitDiff {
            files: self.files,
            trailing_preamble,
        }
    }
}

impl GitDiff {
    /// Parse a unified diff string into structured data.
    pub fn parse(diff: &str) -> Self {
        let mut state = ParseState::new();

        for line in diff.lines() {
            // Strip ANSI codes for pattern matching, but preserve raw line for preamble
            let stripped = strip_ansi(line);

            // "diff --git" starts a new file entry
            if stripped.starts_with("diff --git ") {
                // Finish any pending pure rename before starting new file
                state.finish_pending_rename();
                state.finish_file();
                continue;
            }

            // Parse index line for blob hashes
            if let Some(caps) = index_re().captures(&stripped) {
                state.pending_old_hash = Some(caps.get(1).unwrap().as_str().to_string());
                state.pending_new_hash = Some(caps.get(2).unwrap().as_str().to_string());
                continue;
            }
            if let Some(path) = stripped.strip_prefix("rename from ") {
                state.rename_from = Some(path.to_string());
                continue;
            }
            if let Some(path) = stripped.strip_prefix("rename to ") {
                state.pending_rename_to = Some(path.to_string());
                continue;
            }
            if let Some(path) = stripped.strip_prefix("--- ") {
                state.finish_file();
                state.old_path = strip_prefix_ab(path);
            } else if let Some(path) = stripped.strip_prefix("+++ ") {
                let new_path = strip_prefix_ab(path);
                state.current_file = Some(FileDiff {
                    preamble: std::mem::take(&mut state.pending_preamble),
                    old_path: state.old_path.clone(),
                    new_path,
                    rename_from: state.rename_from.take(),
                    old_hash: state.pending_old_hash.take(),
                    new_hash: state.pending_new_hash.take(),
                    hunks: Vec::new(),
                });
            } else if let Some(caps) = hunk_header_re().captures(&stripped) {
                state.finish_hunk();

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

                state.current_hunk = Some(RawHunk {
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    lines: Vec::new(),
                });
            } else if state.current_hunk.is_some() {
                // Hunk content uses stripped line (gets re-rendered with syntax highlighting)
                let _ = state.handle_in_hunk_line(&stripped);
            } else {
                // Non-diff line before any file starts (commit metadata, etc.)
                // Preserve raw line with ANSI codes for colored output
                state.pending_preamble.push(line.to_string());
            }
        }

        state.into_diff()
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
    fn parse_hunk_body_line_skips_no_newline_marker() {
        assert!(matches!(
            parse_hunk_body_line("\\ No newline at end of file"),
            HunkBodyEvent::Skip
        ));
    }

    #[test]
    fn finish_hunk_moves_the_current_hunk_into_the_current_file() {
        let mut state = ParseState::new();
        state.current_file = Some(FileDiff {
            preamble: Vec::new(),
            old_path: "old.rs".to_string(),
            new_path: "new.rs".to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: Vec::new(),
        });
        state.current_hunk = Some(RawHunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![RawLine {
                kind: RawLineKind::Context,
                content: "line".to_string(),
            }],
        });

        state.finish_hunk();

        assert!(state.current_hunk.is_none());
        assert_eq!(state.current_file.unwrap().hunks.len(), 1);
    }

    #[test]
    fn push_raw_line_adds_it_to_the_current_hunk() {
        let mut state = ParseState::new();
        state.current_hunk = Some(RawHunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: Vec::new(),
        });

        state.push_raw_line(RawLine {
            kind: RawLineKind::Added,
            content: "new".to_string(),
        });

        assert_eq!(state.current_hunk.unwrap().lines.len(), 1);
    }

    #[test]
    fn apply_hunk_body_event_collects_preamble_after_hunk() {
        let mut state = ParseState::new();
        state.current_file = Some(FileDiff {
            preamble: Vec::new(),
            old_path: "old.rs".to_string(),
            new_path: "new.rs".to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: Vec::new(),
        });
        state.current_hunk = Some(RawHunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: Vec::new(),
        });

        let should_continue =
            state.apply_hunk_body_event(HunkBodyEvent::Preamble("commit x".into()));

        assert!(should_continue);
        assert!(state.current_hunk.is_none());
        assert_eq!(state.pending_preamble, vec!["commit x"]);
    }

    #[test]
    fn handle_in_hunk_line_pushes_raw_lines() {
        let mut state = ParseState::new();
        state.current_hunk = Some(RawHunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: Vec::new(),
        });

        let should_continue = state.handle_in_hunk_line("+new");

        assert!(!should_continue);
        assert_eq!(state.current_hunk.unwrap().lines.len(), 1);
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

    #[test]
    fn parse_commit_metadata() {
        // git show format with commit metadata
        let diff = r#"commit abc1234567890
Author: Test User <test@example.com>
Date:   Mon Jan 1 12:00:00 2024 +0000

    Add feature X

diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old
+new
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].preamble.len(), 6);
        assert_eq!(parsed.files[0].preamble[0], "commit abc1234567890");
        assert_eq!(
            parsed.files[0].preamble[1],
            "Author: Test User <test@example.com>"
        );
        assert!(parsed.files[0].preamble[4].contains("Add feature X"));
    }

    #[test]
    fn parse_multi_commit() {
        // git log -p format with multiple commits
        let diff = r#"commit abc1234
Author: User1 <user1@example.com>
Date:   Mon Jan 1 12:00:00 2024 +0000

    First commit

diff --git a/file1.rs b/file1.rs
--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old1
+new1

commit def5678
Author: User2 <user2@example.com>
Date:   Tue Jan 2 12:00:00 2024 +0000

    Second commit

diff --git a/file2.rs b/file2.rs
--- a/file2.rs
+++ b/file2.rs
@@ -1 +1 @@
-old2
+new2
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 2);
        // First file has first commit's metadata
        assert!(parsed.files[0].preamble[0].contains("abc1234"));
        assert!(parsed.files[0].preamble[1].contains("User1"));
        // Second file has second commit's metadata
        assert!(parsed.files[1].preamble[0].contains("def5678"));
        assert!(parsed.files[1].preamble[1].contains("User2"));
    }

    #[test]
    fn parse_diff_no_preamble() {
        // Plain git diff has no preamble
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
        assert!(parsed.files[0].preamble.is_empty());
    }

    #[test]
    fn parse_pure_rename_no_content_change() {
        // 100% similarity rename has no --- / +++ lines and no hunks
        let diff = r#"diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
"#;
        let parsed = GitDiff::parse(diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].old_path, "old.txt");
        assert_eq!(parsed.files[0].new_path, "new.txt");
        assert_eq!(parsed.files[0].rename_from, Some("old.txt".to_string()));
        assert!(parsed.files[0].hunks.is_empty());
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\x1b[33mcommit abc\x1b[0m"), "commit abc");
        assert_eq!(strip_ansi("\x1b[1mdiff --git\x1b[m"), "diff --git");
        assert_eq!(strip_ansi("no codes here"), "no codes here");
        // Multiple codes in sequence
        assert_eq!(
            strip_ansi("\x1b[1;31mred bold\x1b[0m normal"),
            "red bold normal"
        );
    }

    #[test]
    fn parse_preserves_ansi_in_preamble() {
        let input = "\x1b[33mcommit abc123\x1b[0m\n\
                     Author: Test\n\
                     \n\
                     diff --git a/f.rs b/f.rs\n\
                     --- a/f.rs\n\
                     +++ b/f.rs\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new\n";
        let parsed = GitDiff::parse(input);
        assert_eq!(parsed.files.len(), 1);
        // Preamble should preserve ANSI codes
        assert_eq!(parsed.files[0].preamble[0], "\x1b[33mcommit abc123\x1b[0m");
    }

    #[test]
    fn parse_handles_colored_diff_markers() {
        let input = "\x1b[1mdiff --git a/f.rs b/f.rs\x1b[m\n\
                     \x1b[1m--- a/f.rs\x1b[m\n\
                     \x1b[1m+++ b/f.rs\x1b[m\n\
                     \x1b[36m@@ -1,1 +1,1 @@\x1b[m\n\
                     \x1b[31m-old\x1b[m\n\
                     \x1b[32m+new\x1b[m\n";
        let parsed = GitDiff::parse(input);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].new_path, "f.rs");
        assert_eq!(parsed.files[0].hunks.len(), 1);
        assert_eq!(parsed.files[0].hunks[0].lines.len(), 2);
    }
}
