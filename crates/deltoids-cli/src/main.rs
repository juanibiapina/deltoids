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
use deltoids::structural::{SummaryOptions, format_summary_with};
use deltoids::{content, git};

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

/// CLI flags. The default view (no flags) is the historical full-diff
/// behaviour. Flags layer on:
///
/// - `--summary`: replace the diff with a structural summary.
/// - `--summary-then-diff`: print summary first, then the full diff.
/// - `--public`: only files / changes touching public symbols. Filters
///   both the summary (if any) and the diff bodies.
/// - `--signatures-only`: drop body-only changes from the summary;
///   the diff still renders fully. Useful for API-review.
#[derive(Debug, Clone, Default)]
struct Args {
    summary: bool,
    summary_then_diff: bool,
    public_only: bool,
    signatures_only: bool,
    show_help: bool,
}

impl Args {
    fn parse(argv: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut args = Args::default();
        for arg in argv.into_iter().skip(1) {
            match arg.as_str() {
                "--summary" | "-s" => args.summary = true,
                "--summary-then-diff" | "-S" => args.summary_then_diff = true,
                "--public" | "-p" => args.public_only = true,
                "--signatures-only" => args.signatures_only = true,
                "--help" | "-h" => args.show_help = true,
                _ => return Err(format!("deltoids: unknown argument `{arg}`")),
            }
        }
        Ok(args)
    }

    /// True when the structural summary should be printed.
    fn print_summary(&self) -> bool {
        self.summary || self.summary_then_diff
    }

    /// True when the unified diff should be printed.
    fn print_diff(&self) -> bool {
        !self.summary || self.summary_then_diff
    }
}

const HELP_TEXT: &str = "\
deltoids - syntax-aware diff filter

Usage: deltoids [OPTIONS]

  Reads a unified diff on stdin, enriches it with tree-sitter scope
  context, and prints rendered output to stdout. Designed to pipe:

      git diff | deltoids | less -R

OPTIONS:
  -s, --summary            Print a structural summary instead of the
                           diff (lists added / removed / modified
                           named declarations).
  -S, --summary-then-diff  Print the summary first, then the full diff.
  -p, --public             Restrict output to changes touching public
                           symbols. Both summary and diff are filtered.
      --signatures-only    Drop body-only changes from the summary;
                           the diff still renders fully (use this with
                           --summary for an API-change view).
  -h, --help               Show this help.
";

fn main() {
    let args = match Args::parse(std::env::args()) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}\n{HELP_TEXT}");
            std::process::exit(2);
        }
    };
    if args.show_help {
        print!("{HELP_TEXT}");
        return;
    }

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

    let output = match process_diff(&input, width, fill, &theme, &args) {
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
fn render(
    resolved: &ResolvedDiff<'_>,
    width: usize,
    fill: BgFill,
    theme: &Theme,
    args: &Args,
) -> String {
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

        // Public-only filter: skip the entire file if no change touches
        // a public symbol. Keeps the diff aligned with the summary.
        if args.public_only {
            let structural = diff.structural();
            let any_public = structural.public_changes().next().is_some();
            if !any_public {
                continue;
            }
        }

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

        if args.print_summary() {
            output.push('\n');
            append_file_summary(&mut output, &before_content, &after_content, path, args);
        }

        // Render each hunk with breadcrumb box
        if args.print_diff() {
            append_hunks(&mut output, &diff, width, fill, theme);
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
    args: &Args,
) -> Result<String, DiffError> {
    let parsed = GitDiff::parse(input);
    let repo = git::Repo::discover();
    let resolved = resolve(&parsed, repo.as_ref())?;
    Ok(render(&resolved, width, fill, theme, args))
}

/// Append every hunk in `diff` to `out`. One blank line precedes each
/// hunk — callers don't need to add their own. When a hunk falls
/// inside a symbol that has a structural change, an ANSI-coloured
/// label is rendered just above the hunk ("+ Added function `parse`").
fn append_hunks(out: &mut String, diff: &Diff, width: usize, fill: BgFill, theme: &Theme) {
    let structural = diff.structural();
    let span_index = build_change_span_index(&structural);
    for hunk in diff.hunks() {
        out.push('\n');
        if let Some(change) = annotate_for_hunk(hunk, &span_index)
            && let Some(line) = ansi_annotation_line(change, theme)
        {
            out.push_str(&line);
            out.push('\n');
        }
        let hunk_lines = render_hunk(hunk, diff.language(), width, fill, theme);
        for line in hunk_lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
}

/// Build (LineSpan, change) index for one file's structural diff.
fn build_change_span_index(
    structural: &deltoids::StructuralDiff,
) -> Vec<(deltoids::LineSpan, &deltoids::structural::StructuralChange)> {
    structural
        .changes()
        .iter()
        .filter_map(|c| {
            let span = c
                .after
                .as_ref()
                .map(|s| s.span)
                .or_else(|| c.before.as_ref().map(|s| s.span));
            span.map(|s| (s, c))
        })
        .collect()
}

/// Find the smallest span that overlaps `hunk` on the new side.
fn annotate_for_hunk<'a>(
    hunk: &deltoids::Hunk,
    index: &'a [(
        deltoids::LineSpan,
        &'a deltoids::structural::StructuralChange,
    )],
) -> Option<&'a deltoids::structural::StructuralChange> {
    use deltoids::LineKind;
    let new_count = hunk
        .lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Added | LineKind::Context))
        .count();
    let h_start = hunk.new_start.max(1);
    let h_end = if new_count == 0 {
        h_start
    } else {
        h_start + new_count - 1
    };
    let mut best: Option<(usize, &deltoids::structural::StructuralChange)> = None;
    for (span, change) in index {
        if span.end < h_start || span.start > h_end {
            continue;
        }
        let width = span.end.saturating_sub(span.start);
        if best.map(|(w, _)| width < w).unwrap_or(true) {
            best = Some((width, change));
        }
    }
    best.map(|(_, c)| c)
}

/// Render an ANSI-coloured annotation line for the change. Returns
/// `None` for body-only changes (the breadcrumb already names the
/// symbol; the label would be noise).
fn ansi_annotation_line(
    change: &deltoids::structural::StructuralChange,
    theme: &Theme,
) -> Option<String> {
    use deltoids::config::{rgb_to_ansi_bg, rgb_to_ansi_fg};
    use deltoids::structural::ChangeKind;
    if matches!(change.kind, ChangeKind::BodyChanged) {
        return None;
    }
    let (bullet, fg_rgb): (char, (u8, u8, u8)) = match change.kind {
        ChangeKind::Added => ('+', (114, 196, 110)), // green
        ChangeKind::Removed => ('-', (219, 88, 96)), // red
        ChangeKind::Renamed => ('→', theme.muted),
        _ => ('~', (224, 175, 104)), // yellow
    };
    let _ = rgb_to_ansi_bg; // kept available, currently unused here
    let bullet_color = rgb_to_ansi_fg(fg_rgb.0, fg_rgb.1, fg_rgb.2);
    let muted_color = rgb_to_ansi_fg(theme.muted.0, theme.muted.1, theme.muted.2);
    Some(format!(
        "  {bullet_color}{bullet}\x1b[0m {muted_color}{}\x1b[0m",
        change.description
    ))
}

/// Append the structural summary for one file pair to `out`. Skips the
/// title — we already printed a file header so the summary just lists
/// changes.
fn append_file_summary(out: &mut String, before: &str, after: &str, path: &str, args: &Args) {
    let structural = deltoids::StructuralDiff::compute(before, after, path);
    if structural.is_empty() {
        return;
    }
    let opts = SummaryOptions {
        indent: "  ",
        title: false,
        public_only: args.public_only,
        signatures_only: args.signatures_only,
        show_signatures: false,
    };
    let body = format_summary_with(&structural, &opts);
    if !body.trim().is_empty() {
        out.push_str(&body);
    }
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
