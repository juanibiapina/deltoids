//! deltoids - A diff filter with tree-sitter scope context.
//!
//! Reads unified diff from stdin, enriches hunks with structural scope
//! information, and renders with syntax highlighting and breadcrumb boxes.
//!
//! Usage:
//!   git diff | deltoids | less -R
//!   git config core.pager 'deltoids | less -R'

use std::io::{self, Read, Write};

use regex::Regex;

use deltoids::Diff;
use deltoids::parse::GitDiff;
use deltoids::render::{render_file_header, render_hunk, render_rename_header};

mod git {
    use git2::{Oid, Repository};

    pub struct Repo(Repository);

    impl Repo {
        /// Discover git repository from current directory.
        pub fn discover() -> Option<Self> {
            Repository::discover(".").ok().map(Repo)
        }

        /// Get blob content by hash (abbreviated or full).
        /// Returns None if hash is null or blob not found.
        pub fn blob(&self, hash: &str) -> Option<String> {
            if is_null_hash(hash) {
                return None;
            }

            // Try to parse as full OID first
            let oid = Oid::from_str(hash)
                .ok()
                // If that fails, try to resolve abbreviated hash
                .or_else(|| self.0.revparse_single(hash).ok().map(|obj| obj.id()))?;

            let blob = self.0.find_blob(oid).ok()?;
            std::str::from_utf8(blob.content()).ok().map(String::from)
        }
    }

    /// Check if hash represents "no file" (all zeros).
    pub fn is_null_hash(hash: &str) -> bool {
        !hash.is_empty() && hash.chars().all(|c| c == '0')
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
    }
}

mod content {
    use super::git::{is_null_hash, Repo};
    use deltoids::parse::FileDiff;
    use std::fs;

    pub struct FileContent {
        pub before: Option<String>,
        pub after: Option<String>,
    }

    /// Retrieve before/after content for a file diff.
    pub fn retrieve(file: &FileDiff, repo: Option<&Repo>) -> FileContent {
        let before = retrieve_before(file, repo);
        let after = retrieve_after(file, repo);
        FileContent { before, after }
    }

    fn retrieve_before(file: &FileDiff, repo: Option<&Repo>) -> Option<String> {
        // Check if old_hash indicates no file (new file case)
        if let Some(ref hash) = file.old_hash
            && is_null_hash(hash)
        {
            return None;
        }

        // Try to get from git by hash
        if let Some(ref hash) = file.old_hash
            && let Some(repo) = repo
            && let Some(content) = repo.blob(hash)
        {
            return Some(content);
        }

        // Fallback: reconstruct from after content and diff
        // This handles non-git diffs or when blob lookup fails
        let after = fs::read_to_string(&file.new_path).ok()?;
        Some(deltoids::reverse::reconstruct_before(&after, file))
    }

    fn retrieve_after(file: &FileDiff, repo: Option<&Repo>) -> Option<String> {
        // Check if new_hash indicates no file (deleted file case)
        if let Some(ref hash) = file.new_hash
            && is_null_hash(hash)
        {
            return None;
        }

        // Try to get from git by hash first (for committed diffs)
        if let Some(ref hash) = file.new_hash
            && let Some(repo) = repo
            && let Some(content) = repo.blob(hash)
        {
            return Some(content);
        }

        // Fallback: read from filesystem (working tree state)
        fs::read_to_string(&file.new_path).ok()
    }
}

const DEFAULT_WIDTH: usize = 120;

fn main() {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("Failed to read stdin");

    if input.is_empty() {
        return;
    }

    // Strip ANSI escape codes (git sends colored output to pagers)
    let input = strip_ansi(&input);

    let width = terminal_width().unwrap_or(DEFAULT_WIDTH);
    let output = process_diff(&input, width);

    print!("{output}");
    let _ = io::stdout().flush();
}

fn strip_ansi(s: &str) -> String {
    // Match ANSI escape sequences: ESC [ ... m (SGR codes)
    let re = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

fn terminal_width() -> Option<usize> {
    // Try COLUMNS env var first (set by some shells)
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(w) = cols.parse()
    {
        return Some(w);
    }

    // Query terminal size - works even when stdin/stdout are pipes
    // by querying /dev/tty on Unix
    terminal_size::terminal_size().map(|(w, _)| w.0 as usize)
}

fn process_diff(input: &str, width: usize) -> String {
    let parsed = GitDiff::parse(input);
    let repo = git::Repo::discover();
    let mut output = String::new();
    let mut first_file = true;

    for file in &parsed.files {
        // Add blank line before file header (except first file)
        if !first_file {
            output.push('\n');
        }
        first_file = false;

        // Retrieve before/after content
        let content = content::retrieve(file, repo.as_ref());

        // If we can't get either content, fall back to raw diff
        let (before_content, after_content) = match (content.before, content.after) {
            (Some(before), Some(after)) => (before, after),
            (None, Some(after)) => (String::new(), after), // New file
            (Some(before), None) => (before, String::new()), // Deleted file
            (None, None) => {
                // Can't get any content, render raw diff
                for line in render_file_header(&file.new_path, width) {
                    output.push_str(&line);
                    output.push('\n');
                }
                output.push_str(&format_raw_hunks(file, width));
                continue;
            }
        };

        // Compute enriched diff using deltoids library
        let diff = Diff::compute(&before_content, &after_content, &file.new_path);

        // Render file header (2 lines)
        for line in render_file_header(&file.new_path, width) {
            output.push_str(&line);
            output.push('\n');
        }

        // Render rename header if this file was renamed
        if let Some(ref old_path) = file.rename_from {
            output.push_str(&render_rename_header(old_path, &file.new_path));
            output.push('\n');
        }

        // Blank line after header, before hunks
        output.push('\n');

        // Render each hunk with breadcrumb box
        for hunk in diff.hunks() {
            let hunk_lines = render_hunk(hunk, &file.new_path, width, hunk.new_start);
            for line in hunk_lines {
                output.push_str(&line);
                output.push('\n');
            }
        }
    }

    output
}

/// Fallback rendering when file can't be read.
fn format_raw_hunks(file: &deltoids::parse::FileDiff, _width: usize) -> String {
    use deltoids::parse::RawLineKind;

    let mut output = String::new();

    for hunk in &file.hunks {
        // Hunk header
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));

        for line in &hunk.lines {
            let prefix = match line.kind {
                RawLineKind::Context => " ",
                RawLineKind::Added => "+",
                RawLineKind::Removed => "-",
            };
            output.push_str(prefix);
            output.push_str(&line.content);
            output.push('\n');
        }
    }

    output
}
