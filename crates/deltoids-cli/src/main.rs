//! deltoids - A diff filter with tree-sitter scope context.
//!
//! Reads unified diff from stdin, enriches hunks with structural scope
//! information, and renders with syntax highlighting and breadcrumb boxes.
//!
//! Usage:
//!   git diff | deltoids | less -R
//!   git config core.pager 'deltoids | less -R'

use std::io::{self, Read, Write};

use deltoids::Diff;
use deltoids::Theme;
use deltoids::parse::GitDiff;
use deltoids::render::{BgFill, render_file_header, render_hunk, render_rename_header};

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
        fn blob_lookup_abbreviated_hash() {
            // This test requires running in a git repo
            let repo = match Repo::discover() {
                Some(r) => r,
                None => return, // Skip if not in a git repo
            };

            // Get HEAD commit's tree to find a known blob
            let head = repo.0.head().unwrap().peel_to_commit().unwrap();
            let tree = head.tree().unwrap();
            let entry = tree.iter().next().unwrap();
            let full_hash = entry.id().to_string();
            let abbrev_hash = &full_hash[..7];

            // Both should resolve to the same content
            let full_content = repo.blob(&full_hash);
            let abbrev_content = repo.blob(abbrev_hash);

            assert!(full_content.is_some(), "full hash should resolve");
            assert!(abbrev_content.is_some(), "abbreviated hash should resolve");
            assert_eq!(
                full_content, abbrev_content,
                "both should return same content"
            );
        }
    }
}

mod content {
    use super::git::{Repo, is_null_hash};
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

    let width = terminal_width().unwrap_or(DEFAULT_WIDTH);
    let fill = bg_fill_mode();
    let theme = Theme::load();
    let output = process_diff(&input, width, fill, &theme);

    // Use write! instead of print! to handle broken pipe gracefully
    // (happens when user quits `less` before we finish writing)
    let mut stdout = io::stdout().lock();
    if let Err(e) = write!(stdout, "{output}")
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("Error writing to stdout: {e}");
    }
    let _ = stdout.flush();
}

/// Determine fill mode based on whether stdout is a TTY.
/// Use space padding when piped (e.g., through `less`), ANSI erase for direct terminal.
fn bg_fill_mode() -> BgFill {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        BgFill::AnsiErase
    } else {
        BgFill::Spaces
    }
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

/// Get the display path for a file diff.
/// For deleted files (new_path is /dev/null), returns old_path.
fn display_path(file: &deltoids::parse::FileDiff) -> &str {
    if file.new_path == "/dev/null" {
        &file.old_path
    } else {
        &file.new_path
    }
}

fn process_diff(input: &str, width: usize, fill: BgFill, theme: &Theme) -> String {
    let parsed = GitDiff::parse(input);
    let repo = git::Repo::discover();
    let mut output = String::new();
    let mut first_file = true;
    let mut has_diff_content = false;

    for file in &parsed.files {
        // Add blank line before file header (except first file with no preamble)
        if !first_file && file.preamble.is_empty() {
            output.push('\n');
        }
        first_file = false;
        has_diff_content = true;

        // Print preamble lines (commit metadata, etc.) unchanged
        for line in &file.preamble {
            output.push_str(line);
            output.push('\n');
        }

        // Blank line between preamble and file header
        if !file.preamble.is_empty() {
            output.push('\n');
        }

        // Retrieve before/after content
        let content = content::retrieve(file, repo.as_ref());

        // If we can't get either content, fall back to raw diff
        let (before_content, after_content) = match (content.before, content.after) {
            (Some(before), Some(after)) => (before, after),
            (None, Some(after)) => (String::new(), after), // New file
            (Some(before), None) => (before, String::new()), // Deleted file
            (None, None) => {
                // Can't get any content, render raw diff
                let path = display_path(file);
                for line in render_file_header(path, width, theme) {
                    output.push_str(&line);
                    output.push('\n');
                }
                // Render rename header if this file was renamed
                if let Some(ref old_path) = file.rename_from {
                    output.push_str(&render_rename_header(old_path, &file.new_path, theme));
                    output.push('\n');
                }
                output.push_str(&format_raw_hunks(file, width));
                continue;
            }
        };

        // Compute enriched diff using deltoids library
        let path = display_path(file);
        let diff = Diff::compute(&before_content, &after_content, path);

        // Render file header (2 lines)
        for line in render_file_header(path, width, theme) {
            output.push_str(&line);
            output.push('\n');
        }

        // Render rename header if this file was renamed
        if let Some(ref old_path) = file.rename_from {
            output.push_str(&render_rename_header(old_path, &file.new_path, theme));
            output.push('\n');
        }

        // Render each hunk with breadcrumb box
        for hunk in diff.hunks() {
            // Blank line before each hunk
            output.push('\n');

            let hunk_lines = render_hunk(hunk, path, width, fill, theme);
            for line in hunk_lines {
                output.push_str(&line);
                output.push('\n');
            }
        }
    }

    // Output trailing preamble (non-diff content at end, or entire non-diff input)
    if let Some(ref trailing) = parsed.trailing_preamble {
        // Add blank line separator if we had diff content
        if has_diff_content {
            output.push('\n');
        }
        for line in trailing {
            output.push_str(line);
            output.push('\n');
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

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::parse::FileDiff;

    fn make_file_diff(old_path: &str, new_path: &str) -> FileDiff {
        FileDiff {
            preamble: vec![],
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: vec![],
        }
    }

    #[test]
    fn display_path_returns_new_path_for_regular_file() {
        let file = make_file_diff("src/lib.rs", "src/lib.rs");
        assert_eq!(display_path(&file), "src/lib.rs");
    }

    #[test]
    fn display_path_returns_old_path_for_deleted_file() {
        let file = make_file_diff("deleted.rs", "/dev/null");
        assert_eq!(display_path(&file), "deleted.rs");
    }

    #[test]
    fn display_path_returns_new_path_for_new_file() {
        let file = make_file_diff("/dev/null", "new_file.rs");
        assert_eq!(display_path(&file), "new_file.rs");
    }
}
