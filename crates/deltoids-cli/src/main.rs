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
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render::{BgFill, render_file_header, render_hunk, render_rename_header};

mod git {
    use git2::{ObjectType, Oid, Repository};

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
}

mod content {
    use super::git::{Repo, blob_hash_matches, is_null_hash};
    use deltoids::parse::FileDiff;
    use std::fs;

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
    ///   2. Look up the blob in the discovered repo's object database.
    ///   3. Verify a candidate against the expected blob hash:
    ///      - `after`: the working-tree file at `file.new_path`.
    ///      - `before`: the diff reverse-applied onto the resolved
    ///        `after`.
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
            && let Some(content) = repo.blob_text(hash)
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
                Some(after) => {
                    SideContent::Resolved(deltoids::reverse::reconstruct_before(after, file))
                }
                None => SideContent::Absent,
            };
        };

        if is_null_hash(hash) {
            return SideContent::Absent;
        }

        if let Some(repo) = repo
            && let Some(content) = repo.blob_text(hash)
        {
            return SideContent::Resolved(content);
        }

        if let Some(after) = after {
            let reconstructed = deltoids::reverse::reconstruct_before(after, file);
            if blob_hash_matches(&reconstructed, hash) {
                return SideContent::Resolved(reconstructed);
            }
        }

        SideContent::Missing {
            hash: hash.to_string(),
        }
    }
}

/// A blob referenced by a diff that could not be resolved locally.
#[derive(Debug, Clone)]
struct MissingBlob {
    hash: String,
    path: String,
}

/// Errors produced while turning the input diff into rendered output.
#[derive(Debug)]
enum DiffError {
    /// The diff references one or more index blobs that cannot be resolved
    /// locally (neither in the git ODB nor reproducible from the working
    /// tree). The user typically needs to fetch the source ref.
    MissingBlobs(Vec<MissingBlob>),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::MissingBlobs(blobs) => {
                let example = blobs.first().expect("MissingBlobs is never empty");
                let suffix = match blobs.len() {
                    1 => String::new(),
                    n => format!(" (+{} more)", n - 1),
                };
                writeln!(
                    f,
                    "deltoids: missing index blob {} for {}{} — not found in local repository",
                    example.hash, example.path, suffix
                )?;
                writeln!(
                    f,
                    "hint: fetch the source ref (e.g. `git fetch <remote> <ref>`) and try again"
                )?;
                Ok(())
            }
        }
    }
}

impl std::error::Error for DiffError {}

const DEFAULT_WIDTH: usize = 120;

fn main() {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("deltoids: failed to read stdin: {e}");
        std::process::exit(1);
    }

    if input.is_empty() {
        return;
    }

    let width = terminal_width().unwrap_or(DEFAULT_WIDTH);
    let fill = bg_fill_mode();
    let theme = Theme::load();

    let output = match process_diff(&input, width, fill, &theme) {
        Ok(out) => out,
        Err(err) => {
            eprint!("{err}");
            std::process::exit(1);
        }
    };

    // Use write! instead of print! to handle broken pipe gracefully
    // (happens when user quits `less` before we finish writing)
    let mut stdout = io::stdout().lock();
    if let Err(e) = write!(stdout, "{output}")
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("deltoids: error writing to stdout: {e}");
        std::process::exit(1);
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
fn display_path(file: &FileDiff) -> &str {
    if file.new_path == "/dev/null" {
        &file.old_path
    } else {
        &file.new_path
    }
}

/// Resolved content for a single file in the diff. Built by `resolve` and
/// then consumed by `render`.
struct ResolvedFile<'a> {
    file: &'a FileDiff,
    content: content::FileContent,
}

/// Everything `render` needs from `resolve`: per-file resolved content
/// plus the diff's trailing preamble (non-diff text after the last file,
/// kept verbatim).
struct ResolvedDiff<'a> {
    files: Vec<ResolvedFile<'a>>,
    trailing: Option<&'a [String]>,
}

/// Resolve content for every file. Returns the resolved diff on success,
/// or a list of missing blobs covering all affected files on failure.
fn resolve<'a>(
    parsed: &'a GitDiff,
    repo: Option<&git::Repo>,
) -> Result<ResolvedDiff<'a>, DiffError> {
    let mut files = Vec::with_capacity(parsed.files.len());
    let mut missing = Vec::new();

    for file in &parsed.files {
        let content = content::retrieve(file, repo);
        let path = display_path(file);
        if let content::SideContent::Missing { hash } = &content.before {
            missing.push(MissingBlob {
                hash: hash.clone(),
                path: path.to_string(),
            });
        }
        if let content::SideContent::Missing { hash } = &content.after {
            missing.push(MissingBlob {
                hash: hash.clone(),
                path: path.to_string(),
            });
        }
        files.push(ResolvedFile { file, content });
    }

    if !missing.is_empty() {
        return Err(DiffError::MissingBlobs(missing));
    }
    Ok(ResolvedDiff {
        files,
        trailing: parsed.trailing_preamble.as_deref(),
    })
}

/// Render the resolved diff.
fn render(resolved: &ResolvedDiff<'_>, width: usize, fill: BgFill, theme: &Theme) -> String {
    use content::SideContent;

    let mut output = String::new();
    let mut first_file = true;
    let mut has_diff_content = false;

    for r in &resolved.files {
        let file = r.file;

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

        // Determine before/after text. After `resolve` succeeds we never
        // see `Missing`; both sides are either `Resolved` or `Absent`.
        let (before_content, after_content) = match (&r.content.before, &r.content.after) {
            (SideContent::Resolved(b), SideContent::Resolved(a)) => (b.clone(), a.clone()),
            (SideContent::Absent, SideContent::Resolved(a)) => (String::new(), a.clone()),
            (SideContent::Resolved(b), SideContent::Absent) => (b.clone(), String::new()),
            (SideContent::Absent, SideContent::Absent) => {
                // Both sides absent (no hashes given, file not on disk).
                // Fall back to raw hunks straight from the parsed diff.
                let path = display_path(file);
                for line in render_file_header(path, width, theme) {
                    output.push_str(&line);
                    output.push('\n');
                }
                if let Some(ref old_path) = file.rename_from {
                    output.push_str(&render_rename_header(old_path, &file.new_path, theme));
                    output.push('\n');
                }
                output.push_str(&format_raw_hunks(file));
                continue;
            }
            (SideContent::Missing { .. }, _) | (_, SideContent::Missing { .. }) => {
                debug_assert!(false, "Missing should be filtered out by resolve()");
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

            let hunk_lines = render_hunk(hunk, diff.language(), width, fill, theme);
            for line in hunk_lines {
                output.push_str(&line);
                output.push('\n');
            }
        }
    }

    // Output trailing preamble (non-diff content at end, or entire non-diff input)
    if let Some(trailing) = resolved.trailing {
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

fn process_diff(
    input: &str,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Result<String, DiffError> {
    let parsed = GitDiff::parse(input);
    let repo = git::Repo::discover();
    let resolved = resolve(&parsed, repo.as_ref())?;
    Ok(render(&resolved, width, fill, theme))
}

/// Fallback rendering for files where neither side could be resolved.
fn format_raw_hunks(file: &FileDiff) -> String {
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

    #[test]
    fn diff_error_display_is_concise_and_shows_first_missing() {
        let err = DiffError::MissingBlobs(vec![
            MissingBlob {
                hash: "a0c0885f56b".to_string(),
                path: "tools/foo.ts".to_string(),
            },
            MissingBlob {
                hash: "deadbeefdead".to_string(),
                path: "tools/bar.rs".to_string(),
            },
        ]);
        let msg = err.to_string();
        // At most two lines: summary + hint.
        assert_eq!(
            msg.trim_end_matches('\n').lines().count(),
            2,
            "expected exactly two lines, got:\n{msg}"
        );
        assert!(msg.contains("a0c0885f56b"));
        assert!(msg.contains("tools/foo.ts"));
        assert!(msg.contains("+1 more"));
        assert!(msg.contains("git fetch"));
        // Don't enumerate every entry.
        assert!(!msg.contains("deadbeefdead"));
        assert!(!msg.contains("tools/bar.rs"));
    }

    #[test]
    fn diff_error_display_omits_more_suffix_for_single_missing() {
        let err = DiffError::MissingBlobs(vec![MissingBlob {
            hash: "a0c0885f56b".to_string(),
            path: "tools/foo.ts".to_string(),
        }]);
        let msg = err.to_string();
        assert!(!msg.contains("more"), "got:\n{msg}");
    }
}
