//! deltoids - A diff filter with tree-sitter scope context.
//!
//! Reads unified diff from stdin, enriches hunks with structural scope
//! information, and renders with syntax highlighting and breadcrumb boxes.
//!
//! Usage:
//!   git diff | deltoids | less -R
//!   git config core.pager 'deltoids | less -R'

use std::fs;
use std::io::{self, Read, Write};
use std::process::Command;

use regex::Regex;

use deltoids::Diff;
use deltoids::parse::ParsedDiff;
use deltoids::render::{render_file_header, render_hunk};
use deltoids::reverse::reconstruct_before;

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
    // Try to get terminal width from environment or terminal query
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(w) = cols.parse()
    {
        return Some(w);
    }

    // Fallback: try stty (suppress stderr for non-TTY contexts)
    if let Ok(output) = Command::new("stty")
        .arg("size")
        .stderr(std::process::Stdio::null())
        .output()
        && let Ok(s) = String::from_utf8(output.stdout)
        && let Some(cols) = s.split_whitespace().nth(1)
        && let Ok(w) = cols.parse()
    {
        return Some(w);
    }

    None
}

fn process_diff(input: &str, width: usize) -> String {
    let parsed = ParsedDiff::parse(input);
    let mut output = String::new();

    for file in &parsed.files {
        // Read current file content (the "after" state)
        let after_content = match fs::read_to_string(&file.new_path) {
            Ok(content) => content,
            Err(_) => {
                // File doesn't exist or can't be read, fall back to raw diff
                output.push_str(&render_file_header(&file.new_path, width));
                output.push('\n');
                output.push_str(&format_raw_hunks(file, width));
                continue;
            }
        };

        // Reconstruct the "before" content
        let before_content = reconstruct_before(&after_content, file);

        // Compute enriched diff using deltoids library
        let diff = Diff::compute(&before_content, &after_content, &file.new_path);

        // Render file header
        output.push_str(&render_file_header(&file.new_path, width));
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


