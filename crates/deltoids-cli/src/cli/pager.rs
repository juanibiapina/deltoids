//! `deltoids pager` — ANSI diff filter with tree-sitter scope context.
//!
//! Reads a unified diff on stdin, enriches hunks with structural scope
//! information, and renders with syntax highlighting and breadcrumb
//! boxes. This is the canonical pager body for `git config core.pager
//! 'deltoids | less -R'` (and the no-subcommand default of the
//! `deltoids` binary when stdin is a pipe).

use std::io::{self, Read, Write};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use deltoids::Diff;
use deltoids::Theme;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render::{BgFill, render_file_header, render_hunk, render_rename_header};
use deltoids::{content, git};

const OVERVIEW: &str = r#"Read a unified diff on stdin, render it with deltoids, and write to stdout.

Examples:
  git diff | deltoids pager | less -R
  git show HEAD~1 | deltoids pager | less -R

Tip: `deltoids` (no subcommand) runs the pager when stdin is a pipe,
so existing `core.pager 'deltoids | less -R'` configurations keep
working unchanged.
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {}

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

pub fn run(_args: Args) -> ExitCode {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("deltoids: failed to read stdin: {e}");
        return ExitCode::from(1);
    }

    if input.is_empty() {
        return ExitCode::SUCCESS;
    }

    let width = terminal_width().unwrap_or(DEFAULT_WIDTH);
    let fill = bg_fill_mode();
    let theme = Theme::load();

    let output = match process_diff(&input, width, fill, &theme) {
        Ok(out) => out,
        Err(err) => {
            eprint!("{err}");
            return ExitCode::from(1);
        }
    };

    // Use write! instead of print! to handle broken pipe gracefully
    // (happens when user quits `less` before we finish writing)
    let mut stdout = io::stdout().lock();
    if let Err(e) = write!(stdout, "{output}")
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("deltoids: error writing to stdout: {e}");
        return ExitCode::from(1);
    }
    let _ = stdout.flush();
    ExitCode::SUCCESS
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

            let hunk_lines = render_hunk(hunk, diff.highlight(), width, fill, theme);
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
