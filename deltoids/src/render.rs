//! Render diff output with ANSI colors and breadcrumb boxes.

use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect_assets::assets::HighlightingAssets;
use unicode_width::UnicodeWidthStr;

use crate::intraline::{compute_subhunk_emphasis, EmphKind, LineEmphasis};
use crate::{Hunk, LineKind, ScopeNode};

const TAB_WIDTH: usize = 4;
const THEME_NAME: &str = "OneHalfDark";

// ANSI color codes
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

// TokyoNight-inspired colors
const BLUE: &str = "\x1b[38;2;122;162;247m"; // RGB(122, 162, 247)
const GREEN_BG: &str = "\x1b[48;2;32;48;59m"; // RGB(0x20, 0x30, 0x3b)
const GREEN_EMPH_BG: &str = "\x1b[48;2;44;90;102m"; // RGB(0x2c, 0x5a, 0x66)
const RED_BG: &str = "\x1b[48;2;55;34;44m"; // RGB(0x37, 0x22, 0x2c)
const RED_EMPH_BG: &str = "\x1b[48;2;113;49;55m"; // RGB(0x71, 0x31, 0x37)

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_syntax_set()
            .expect("integrated syntect assets should load")
            .clone()
    })
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_theme(THEME_NAME)
            .clone()
    })
}

fn syntax_for_path(path: &str) -> &'static SyntaxReference {
    let syntax_set = syntax_set();
    let path = Path::new(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    syntax_set
        .find_syntax_by_extension(file_name)
        .or_else(|| syntax_set.find_syntax_by_extension(extension))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// Render a file header line.
pub fn render_file_header(path: &str, width: usize) -> String {
    let line = format!("{}─── {path} ", BLUE);
    let visible_len = 4 + path.len() + 1; // "─── " + path + " "
    let remaining = width.saturating_sub(visible_len);
    format!("{line}{}{RESET}", "─".repeat(remaining))
}

/// Render a breadcrumb box showing ancestor scopes.
pub fn render_breadcrumb_box(ancestors: &[ScopeNode], path: &str, width: usize) -> Vec<String> {
    if ancestors.is_empty() {
        return Vec::new();
    }

    let max_content_width = width.saturating_sub(2); // room for " │"

    // Compute the widest line number for right-alignment
    let max_line_num = ancestors.iter().map(|a| a.start_line).max().unwrap_or(0);
    let num_col_width = max_line_num.to_string().len();

    // Build content rows: ancestor lines with optional "..." gaps between
    struct Row {
        line_num: Option<usize>,
        text: Option<String>, // None for "..." rows
    }
    let mut rows: Vec<Row> = Vec::new();
    for (i, ancestor) in ancestors.iter().enumerate() {
        if i > 0 {
            let prev = &ancestors[i - 1];
            if prev.start_line + 1 < ancestor.start_line {
                rows.push(Row {
                    line_num: None,
                    text: None,
                });
            }
        }
        rows.push(Row {
            line_num: Some(ancestor.start_line),
            text: Some(ancestor.text.clone()),
        });
    }

    // Compute the widest rendered line for box width
    let prefix_width = num_col_width + 2; // "NNN: "
    let mut max_row_width = 0usize;
    for row in &rows {
        let row_width = match &row.text {
            Some(text) => prefix_width + display_width(text),
            None => prefix_width + 3, // "..."
        };
        max_row_width = max_row_width.max(row_width);
    }
    let content_width = max_row_width.min(max_content_width);

    let top = format!("{BLUE}{}╮{RESET}", "─".repeat(content_width + 1));
    let bot = format!("{BLUE}{}╯{RESET}", "─".repeat(content_width + 1));

    let mut lines = vec![top];

    for row in &rows {
        match &row.text {
            Some(text) => {
                let num_str = format!(
                    "{:>width$}: ",
                    row.line_num.unwrap_or(0),
                    width = num_col_width
                );
                let highlighted = highlight_line(text, path);
                let text_width = display_width(text);
                let padding = content_width.saturating_sub(prefix_width + text_width);

                lines.push(format!(
                    "{BLUE}{num_str}{RESET}{highlighted}{}{BLUE} │{RESET}",
                    " ".repeat(padding)
                ));
            }
            None => {
                // "..." gap row
                let dots = format!("{:>width$}  ...", "", width = num_col_width);
                let padding = content_width.saturating_sub(display_width(&dots));
                lines.push(format!(
                    "{BLUE}{dots}{}{BLUE} │{RESET}",
                    " ".repeat(padding)
                ));
            }
        }
    }

    lines.push(bot);
    lines
}

/// Render a diff line with syntax highlighting and appropriate background.
pub fn render_diff_line(kind: &LineKind, content: &str, path: &str, width: usize) -> String {
    let bg = match kind {
        LineKind::Added => GREEN_BG,
        LineKind::Removed => RED_BG,
        LineKind::Context => "",
    };

    let highlighted = highlight_line(content, path);
    let content_width = display_width(content);
    let padding = width.saturating_sub(content_width);

    if bg.is_empty() {
        format!("{highlighted}{}{RESET}", " ".repeat(padding))
    } else {
        format!("{bg}{highlighted}{}{RESET}", " ".repeat(padding))
    }
}

/// Syntax-highlight a line and return ANSI-escaped string.
/// 
/// Note: This only sets foreground colors, not background. The caller is
/// responsible for setting/resetting background. This allows background
/// colors to persist across all tokens.
pub fn highlight_line(line: &str, path: &str) -> String {
    let syntax = syntax_for_path(path);
    let mut highlighter = HighlightLines::new(syntax, theme());

    match highlighter.highlight_line(line, syntax_set()) {
        Ok(ranges) => {
            let mut result = String::new();
            for (style, text) in ranges {
                // Convert tabs to spaces
                let text = text.replace('\t', &" ".repeat(TAB_WIDTH));

                // Build ANSI escape sequence for foreground only
                // We don't reset here so background colors persist
                let fg = format!(
                    "\x1b[38;2;{};{};{}m",
                    style.foreground.r, style.foreground.g, style.foreground.b
                );

                if style.font_style.contains(FontStyle::BOLD) {
                    result.push_str(BOLD);
                }

                result.push_str(&fg);
                result.push_str(&text);
            }
            result
        }
        Err(_) => {
            // Fallback to plain text
            line.replace('\t', &" ".repeat(TAB_WIDTH))
        }
    }
}

/// Compute display width of a string (handling tabs and unicode).
fn display_width(s: &str) -> usize {
    let mut width = 0;
    for ch in s.chars() {
        if ch == '\t' {
            width += TAB_WIDTH;
        } else {
            width += UnicodeWidthStr::width(ch.to_string().as_str());
        }
    }
    width
}

/// Render a diff line with emphasis sections for intraline highlighting.
pub fn render_diff_line_with_emphasis(
    kind: &LineKind,
    content: &str,
    emphasis: &LineEmphasis,
    path: &str,
    width: usize,
) -> String {
    let (plain_bg, emph_bg) = match kind {
        LineKind::Added => (GREEN_BG, GREEN_EMPH_BG),
        LineKind::Removed => (RED_BG, RED_EMPH_BG),
        LineKind::Context => return render_diff_line(kind, content, path, width),
    };

    match emphasis {
        LineEmphasis::Plain => render_diff_line(kind, content, path, width),
        LineEmphasis::Paired(sections) => {
            let mut result = String::new();
            result.push_str(plain_bg);

            for section in sections {
                let bg = match section.kind {
                    EmphKind::Emph => emph_bg,
                    EmphKind::NonEmph => plain_bg,
                };
                let highlighted = highlight_line(&section.text, path);
                result.push_str(bg);
                result.push_str(&highlighted);
            }

            // Pad to full width
            let content_width = display_width(content);
            let padding = width.saturating_sub(content_width);
            result.push_str(&" ".repeat(padding));

            result.push_str(RESET);
            result
        }
    }
}

/// Render a subhunk (consecutive +/- lines) with intraline emphasis.
///
/// Extracts minus and plus lines, computes emphasis, and renders with
/// word-level highlighting for changed portions.
pub fn render_subhunk(lines: &[(LineKind, &str)], path: &str, width: usize) -> Vec<String> {
    // Separate minus and plus lines
    let mut minus_lines: Vec<&str> = Vec::new();
    let mut plus_lines: Vec<&str> = Vec::new();

    for (kind, content) in lines {
        match kind {
            LineKind::Removed => minus_lines.push(content),
            LineKind::Added => plus_lines.push(content),
            LineKind::Context => {}
        }
    }

    // Compute emphasis
    let (minus_emphasis, plus_emphasis) =
        compute_subhunk_emphasis(&minus_lines, &plus_lines);

    // Render in original order
    let mut output = Vec::new();
    let mut minus_idx = 0;
    let mut plus_idx = 0;

    for (kind, content) in lines {
        match kind {
            LineKind::Removed => {
                output.push(render_diff_line_with_emphasis(
                    kind,
                    content,
                    &minus_emphasis[minus_idx],
                    path,
                    width,
                ));
                minus_idx += 1;
            }
            LineKind::Added => {
                output.push(render_diff_line_with_emphasis(
                    kind,
                    content,
                    &plus_emphasis[plus_idx],
                    path,
                    width,
                ));
                plus_idx += 1;
            }
            LineKind::Context => {
                output.push(render_diff_line(kind, content, path, width));
            }
        }
    }

    output
}

/// Render a full hunk with breadcrumb box and diff lines.
pub fn render_hunk(hunk: &Hunk, path: &str, width: usize, hunk_start: usize) -> Vec<String> {
    let mut output = Vec::new();

    // Render breadcrumb box if we have ancestors
    if !hunk.ancestors.is_empty() {
        // Check if the innermost ancestor is already visible in the diff context
        let scope_expanded = hunk.ancestors.last().is_some_and(|innermost| {
            // If first context line would be >= innermost.start_line, ancestor is visible
            hunk_start >= innermost.start_line
        });

        let ancestors_to_show = if scope_expanded && hunk.ancestors.len() > 1 {
            // Drop innermost ancestor since it's visible in the diff
            &hunk.ancestors[..hunk.ancestors.len() - 1]
        } else if scope_expanded && hunk.ancestors.len() == 1 {
            // Single ancestor already visible, skip breadcrumb entirely
            &[]
        } else {
            &hunk.ancestors[..]
        };

        output.extend(render_breadcrumb_box(ancestors_to_show, path, width));
    }

    // Render diff lines with intraline emphasis for consecutive +/- runs
    let mut i = 0;
    while i < hunk.lines.len() {
        let line = &hunk.lines[i];

        if matches!(line.kind, LineKind::Context) {
            // Context lines render directly
            output.push(render_diff_line(&line.kind, &line.content, path, width));
            i += 1;
        } else {
            // Collect consecutive +/- lines as a subhunk
            let start = i;
            while i < hunk.lines.len()
                && !matches!(hunk.lines[i].kind, LineKind::Context)
            {
                i += 1;
            }

            let subhunk: Vec<(LineKind, &str)> = hunk.lines[start..i]
                .iter()
                .map(|l| (l.kind.clone(), l.content.as_str()))
                .collect();

            output.extend(render_subhunk(&subhunk, path, width));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffLine;

    #[test]
    fn file_header_contains_path() {
        let header = render_file_header("src/main.rs", 80);
        assert!(header.contains("src/main.rs"));
        assert!(header.contains("───"));
    }

    #[test]
    fn breadcrumb_box_has_corners() {
        let ancestors = vec![ScopeNode {
            kind: "function_item".to_string(),
            name: "main".to_string(),
            start_line: 1,
            end_line: 10,
            text: "fn main() {".to_string(),
        }];
        let lines = render_breadcrumb_box(&ancestors, "test.rs", 80);
        assert!(lines[0].contains("╮"));
        assert!(lines.last().unwrap().contains("╯"));
    }

    #[test]
    fn breadcrumb_box_shows_gap_markers() {
        let ancestors = vec![
            ScopeNode {
                kind: "impl_item".to_string(),
                name: "Config".to_string(),
                start_line: 3,
                end_line: 50,
                text: "impl Config {".to_string(),
            },
            ScopeNode {
                kind: "function_item".to_string(),
                name: "process".to_string(),
                start_line: 14,
                end_line: 30,
                text: "    fn process(&self) -> Result<(), Error> {".to_string(),
            },
        ];
        let lines = render_breadcrumb_box(&ancestors, "test.rs", 80);
        let all = lines.join("\n");
        assert!(all.contains("..."), "should have gap marker");
    }

    #[test]
    fn diff_line_added_has_green_bg() {
        let line = render_diff_line(&LineKind::Added, "let x = 1;", "test.rs", 80);
        assert!(line.contains("\x1b[48;2;32;48;59m")); // GREEN_BG
    }

    #[test]
    fn diff_line_removed_has_red_bg() {
        let line = render_diff_line(&LineKind::Removed, "let y = 2;", "test.rs", 80);
        assert!(line.contains("\x1b[48;2;55;34;44m")); // RED_BG
    }

    #[test]
    fn highlight_produces_ansi() {
        let highlighted = highlight_line("fn main() {}", "test.rs");
        assert!(highlighted.contains("\x1b[")); // Contains ANSI codes
    }

    #[test]
    fn render_hunk_drops_visible_ancestor() {
        let hunk = Hunk {
            old_start: 10,
            new_start: 10,
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "fn main() {".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "    println!(\"hello\");".to_string(),
                },
            ],
            ancestors: vec![ScopeNode {
                kind: "function_item".to_string(),
                name: "main".to_string(),
                start_line: 10,
                end_line: 20,
                text: "fn main() {".to_string(),
            }],
        };

        // hunk_start = 10 matches innermost.start_line = 10, so ancestor is visible
        let lines = render_hunk(&hunk, "test.rs", 80, 10);

        // Should not have breadcrumb box since single ancestor is visible
        assert!(!lines.iter().any(|l| l.contains("╮")));
    }

    #[test]
    fn render_subhunk_similar_lines_have_emphasis_bg() {
        // Similar lines should be paired and have emphasis backgrounds
        let lines: Vec<(LineKind, &str)> = vec![
            (LineKind::Removed, "const x = 1;"),
            (LineKind::Added, "const x = 2;"),
        ];
        let output = render_subhunk(&lines, "test.rs", 80);

        // Both lines should have emphasis background for the changed portion
        // GREEN_EMPH_BG = \x1b[48;2;44;90;102m
        // RED_EMPH_BG = \x1b[48;2;113;49;55m
        assert!(output[0].contains("\x1b[48;2;113;49;55m"), "minus should have RED_EMPH_BG");
        assert!(output[1].contains("\x1b[48;2;44;90;102m"), "plus should have GREEN_EMPH_BG");
    }

    #[test]
    fn render_subhunk_dissimilar_lines_plain() {
        // Dissimilar lines should NOT be paired, so no emphasis backgrounds
        let lines: Vec<(LineKind, &str)> = vec![
            (LineKind::Removed, "aaa bbb ccc ddd eee fff ggg hhh"),
            (LineKind::Added, "xxx yyy zzz www uuu vvv ppp qqq"),
        ];
        let output = render_subhunk(&lines, "test.rs", 80);

        // Should have plain backgrounds only, no emphasis
        assert!(!output[0].contains("\x1b[48;2;113;49;55m"), "minus should NOT have RED_EMPH_BG");
        assert!(!output[1].contains("\x1b[48;2;44;90;102m"), "plus should NOT have GREEN_EMPH_BG");
    }
}
