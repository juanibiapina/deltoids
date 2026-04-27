//! Render diff output with ANSI colors and breadcrumb boxes.

use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::parsing::SyntaxReference;
use unicode_width::UnicodeWidthStr;

use crate::config::{SyntaxAssets, Theme, rgb_to_ansi_bg, rgb_to_ansi_fg};
use crate::intraline::{EmphKind, LineEmphasis, compute_subhunk_emphasis};
use crate::{Hunk, HunkRun, LineKind, ScopeNode};

const TAB_WIDTH: usize = 4;

// ANSI color codes
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ERASE_EOL: &str = "\x1b[0K"; // Erase to end of line
const DEFAULT_FG: &str = "\x1b[38;2;220;223;228m"; // Default text color for empty lines

/// Convert a syntect color to an ANSI foreground escape sequence.
///
/// The "ansi" theme encodes ANSI color indices specially:
/// - `r=N, g=0, b=0, a=0` means ANSI color N (0-15)
/// - `r=0, g=0, b=0, a=1` means default foreground
fn syntect_color_to_ansi_fg(color: syntect::highlighting::Color) -> String {
    // ANSI theme encoding: g=0, b=0, a=0 means r is the ANSI color index
    if color.g == 0 && color.b == 0 && color.a == 0 && color.r <= 15 {
        // Use 256-color mode for indices 0-15 (maps to terminal's 16 colors)
        return format!("\x1b[38;5;{}m", color.r);
    }

    // Default foreground (a=1 with black RGB) - reset to default
    if color.r == 0 && color.g == 0 && color.b == 0 && color.a == 1 {
        return "\x1b[39m".to_string(); // Default foreground
    }

    // Actual RGB color
    format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b)
}

/// How to fill background color to end of line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgFill {
    /// Use ANSI CSI sequence (\x1b[0K) - efficient but not supported by `less -R`.
    AnsiErase,
    /// Pad with spaces to terminal width - works through pagers.
    Spaces,
}

fn syntax_for_path(path: &str) -> &'static SyntaxReference {
    let assets = SyntaxAssets::load();
    let syntax_set = assets.syntax_set;
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

/// Render a file header (2 lines: path, then separator).
pub fn render_file_header(path: &str, width: usize, theme: &Theme) -> Vec<String> {
    let separator_fg = rgb_to_ansi_fg(theme.separator.0, theme.separator.1, theme.separator.2);
    vec![
        format!("{BOLD}{path}{RESET}"),
        format!("{separator_fg}{}{RESET}", "─".repeat(width)),
    ]
}

/// Render a rename header showing old ⟶ new path.
pub fn render_rename_header(old_path: &str, new_path: &str, theme: &Theme) -> String {
    let muted_fg = rgb_to_ansi_fg(theme.muted.0, theme.muted.1, theme.muted.2);
    format!("{muted_fg}renamed: {old_path} ⟶ {new_path}{RESET}")
}

/// Render a breadcrumb box showing ancestor scopes.
pub fn render_breadcrumb_box(
    ancestors: &[ScopeNode],
    path: &str,
    width: usize,
    theme: &Theme,
) -> Vec<String> {
    if ancestors.is_empty() {
        return Vec::new();
    }

    let border_fg = rgb_to_ansi_fg(theme.border.0, theme.border.1, theme.border.2);
    let line_num_fg = rgb_to_ansi_fg(
        theme.line_number.0,
        theme.line_number.1,
        theme.line_number.2,
    );
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

    let top = format!("{border_fg}{}┐{RESET}", "─".repeat(content_width + 1));
    let bot = format!("{border_fg}{}┘{RESET}", "─".repeat(content_width + 1));

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
                    "{line_num_fg}{num_str}{RESET}{highlighted}{}{border_fg} │{RESET}",
                    " ".repeat(padding)
                ));
            }
            None => {
                // "..." gap row
                let dots = format!("{:>width$}  ...", "", width = num_col_width);
                let padding = content_width.saturating_sub(display_width(&dots));
                lines.push(format!(
                    "{border_fg}{dots}{}{border_fg} │{RESET}",
                    " ".repeat(padding)
                ));
            }
        }
    }

    lines.push(bot);
    lines
}

/// Render a diff line with syntax highlighting and appropriate background.
pub fn render_diff_line(
    kind: &LineKind,
    content: &str,
    path: &str,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> String {
    let bg = match kind {
        LineKind::Added => rgb_to_ansi_bg(
            theme.diff_added_bg.0,
            theme.diff_added_bg.1,
            theme.diff_added_bg.2,
        ),
        LineKind::Removed => rgb_to_ansi_bg(
            theme.diff_deleted_bg.0,
            theme.diff_deleted_bg.1,
            theme.diff_deleted_bg.2,
        ),
        LineKind::Context => String::new(),
    };

    let highlighted = highlight_line(content, path);

    if bg.is_empty() {
        format!("{highlighted}{RESET}")
    } else {
        let fill_str = bg_fill_string(content, width, fill);
        // For empty lines, set a default foreground color to ensure background renders
        if highlighted.is_empty() {
            format!("{bg}{DEFAULT_FG}{fill_str}{RESET}")
        } else {
            format!("{bg}{highlighted}{fill_str}{RESET}")
        }
    }
}

/// Generate the string to fill background to end of line.
fn bg_fill_string(content: &str, width: usize, fill: BgFill) -> String {
    match fill {
        BgFill::AnsiErase => ERASE_EOL.to_string(),
        BgFill::Spaces => {
            let content_width = display_width(content);
            let padding = width.saturating_sub(content_width);
            " ".repeat(padding)
        }
    }
}

/// Syntax-highlight a line and return ANSI-escaped string.
///
/// Note: This only sets foreground colors, not background. The caller is
/// responsible for setting/resetting background. This allows background
/// colors to persist across all tokens.
pub fn highlight_line(line: &str, path: &str) -> String {
    let assets = SyntaxAssets::load();
    let syntax = syntax_for_path(path);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => {
            let mut result = String::new();
            for (style, text) in ranges {
                // Convert tabs to spaces
                let text = text.replace('\t', &" ".repeat(TAB_WIDTH));

                // Build ANSI escape sequence for foreground only
                // We don't reset here so background colors persist
                //
                // The "ansi" theme encodes ANSI colors specially:
                // - r=N, g=0, b=0, a=0 means ANSI color N (0-15)
                // - r=0, g=0, b=0, a=1 means default foreground
                let fg = syntect_color_to_ansi_fg(style.foreground);

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
    fill: BgFill,
    theme: &Theme,
) -> String {
    let (plain_bg, emph_bg) = match kind {
        LineKind::Added => (
            rgb_to_ansi_bg(
                theme.diff_added_bg.0,
                theme.diff_added_bg.1,
                theme.diff_added_bg.2,
            ),
            rgb_to_ansi_bg(
                theme.diff_added_emph_bg.0,
                theme.diff_added_emph_bg.1,
                theme.diff_added_emph_bg.2,
            ),
        ),
        LineKind::Removed => (
            rgb_to_ansi_bg(
                theme.diff_deleted_bg.0,
                theme.diff_deleted_bg.1,
                theme.diff_deleted_bg.2,
            ),
            rgb_to_ansi_bg(
                theme.diff_deleted_emph_bg.0,
                theme.diff_deleted_emph_bg.1,
                theme.diff_deleted_emph_bg.2,
            ),
        ),
        LineKind::Context => return render_diff_line(kind, content, path, width, fill, theme),
    };

    match emphasis {
        LineEmphasis::Plain => render_diff_line(kind, content, path, width, fill, theme),
        LineEmphasis::Paired(sections) => {
            let mut result = String::new();
            result.push_str(&plain_bg);

            for section in sections {
                let bg = match section.kind {
                    EmphKind::Emph => &emph_bg,
                    EmphKind::NonEmph => &plain_bg,
                };
                let highlighted = highlight_line(&section.text, path);
                result.push_str(bg);
                result.push_str(&highlighted);
            }

            // Reset to plain background before filling to end of line
            result.push_str(&plain_bg);
            // For empty lines, set a default foreground color to ensure background renders
            if content.is_empty() {
                result.push_str(DEFAULT_FG);
            }
            result.push_str(&bg_fill_string(content, width, fill));

            result.push_str(RESET);
            result
        }
    }
}

/// Render a subhunk (consecutive +/- lines) with intraline emphasis.
///
/// Extracts minus and plus lines, computes emphasis, and renders with
/// word-level highlighting for changed portions.
pub fn render_subhunk(
    lines: &[(LineKind, &str)],
    path: &str,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Vec<String> {
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
    let (minus_emphasis, plus_emphasis) = compute_subhunk_emphasis(&minus_lines, &plus_lines);

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
                    fill,
                    theme,
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
                    fill,
                    theme,
                ));
                plus_idx += 1;
            }
            LineKind::Context => {
                output.push(render_diff_line(kind, content, path, width, fill, theme));
            }
        }
    }

    output
}

/// Render a full hunk with breadcrumb box and diff lines.
pub fn render_hunk(
    hunk: &Hunk,
    path: &str,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Vec<String> {
    let mut output = Vec::new();

    // Render breadcrumb box if we have ancestors
    if !hunk.ancestors.is_empty() {
        output.extend(render_breadcrumb_box(&hunk.ancestors, path, width, theme));
    }

    // Render diff lines with intraline emphasis for consecutive +/- runs.
    // `Hunk::runs` already groups change lines for us; we just dispatch.
    for run in hunk.runs() {
        match run {
            HunkRun::Context(line) => {
                output.push(render_diff_line(
                    &line.kind,
                    &line.content,
                    path,
                    width,
                    fill,
                    theme,
                ));
            }
            HunkRun::Change(slice) => {
                let subhunk: Vec<(LineKind, &str)> = slice
                    .iter()
                    .map(|l| (l.kind.clone(), l.content.as_str()))
                    .collect();
                output.extend(render_subhunk(&subhunk, path, width, fill, theme));
            }
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
        let theme = Theme::default();
        let header = render_file_header("src/main.rs", 80, &theme);
        assert_eq!(header.len(), 2);
        assert!(header[0].contains("src/main.rs"));
        assert!(header[1].contains("───"));
    }

    #[test]
    fn breadcrumb_box_has_corners() {
        let theme = Theme::default();
        let ancestors = vec![ScopeNode {
            kind: "function_item".to_string(),
            name: "main".to_string(),
            start_line: 1,
            end_line: 10,
            text: "fn main() {".to_string(),
        }];
        let lines = render_breadcrumb_box(&ancestors, "test.rs", 80, &theme);
        assert!(lines[0].contains("┐"));
        assert!(lines.last().unwrap().contains("┘"));
    }

    #[test]
    fn breadcrumb_box_shows_gap_markers() {
        let theme = Theme::default();
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
        let lines = render_breadcrumb_box(&ancestors, "test.rs", 80, &theme);
        let all = lines.join("\n");
        assert!(all.contains("..."), "should have gap marker");
    }

    #[test]
    fn diff_line_added_has_green_bg() {
        let theme = Theme::default();
        let line = render_diff_line(
            &LineKind::Added,
            "let x = 1;",
            "test.rs",
            80,
            BgFill::AnsiErase,
            &theme,
        );
        assert!(line.contains("\x1b[48;2;32;48;59m")); // GREEN_BG
    }

    #[test]
    fn diff_line_removed_has_red_bg() {
        let theme = Theme::default();
        let line = render_diff_line(
            &LineKind::Removed,
            "let y = 2;",
            "test.rs",
            80,
            BgFill::AnsiErase,
            &theme,
        );
        assert!(line.contains("\x1b[48;2;55;34;44m")); // RED_BG
    }

    #[test]
    fn highlight_produces_ansi() {
        let highlighted = highlight_line("fn main() {}", "test.rs");
        assert!(highlighted.contains("\x1b[")); // Contains ANSI codes
    }

    #[test]
    fn render_hunk_shows_all_ancestors() {
        let theme = Theme::default();
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

        let lines = render_hunk(&hunk, "test.rs", 80, BgFill::AnsiErase, &theme);

        // Should have breadcrumb box with ancestor even if visible in diff
        assert!(lines.iter().any(|l| l.contains("┐")));
    }

    #[test]
    fn render_subhunk_similar_lines_have_emphasis_bg() {
        let theme = Theme::default();
        // Similar lines should be paired and have emphasis backgrounds
        let lines: Vec<(LineKind, &str)> = vec![
            (LineKind::Removed, "const x = 1;"),
            (LineKind::Added, "const x = 2;"),
        ];
        let output = render_subhunk(&lines, "test.rs", 80, BgFill::AnsiErase, &theme);

        // Both lines should have emphasis background for the changed portion
        // GREEN_EMPH_BG = \x1b[48;2;44;90;102m
        // RED_EMPH_BG = \x1b[48;2;113;49;55m
        assert!(
            output[0].contains("\x1b[48;2;113;49;55m"),
            "minus should have RED_EMPH_BG"
        );
        assert!(
            output[1].contains("\x1b[48;2;44;90;102m"),
            "plus should have GREEN_EMPH_BG"
        );
    }

    #[test]
    fn render_subhunk_dissimilar_lines_plain() {
        let theme = Theme::default();
        // Dissimilar lines should NOT be paired, so no emphasis backgrounds
        let lines: Vec<(LineKind, &str)> = vec![
            (LineKind::Removed, "aaa bbb ccc ddd eee fff ggg hhh"),
            (LineKind::Added, "xxx yyy zzz www uuu vvv ppp qqq"),
        ];
        let output = render_subhunk(&lines, "test.rs", 80, BgFill::AnsiErase, &theme);

        // Should have plain backgrounds only, no emphasis
        assert!(
            !output[0].contains("\x1b[48;2;113;49;55m"),
            "minus should NOT have RED_EMPH_BG"
        );
        assert!(
            !output[1].contains("\x1b[48;2;44;90;102m"),
            "plus should NOT have GREEN_EMPH_BG"
        );
    }

    #[test]
    fn rename_header_shows_arrow() {
        let theme = Theme::default();
        let header = render_rename_header("old/path.rs", "new/path.rs", &theme);
        assert!(header.contains("old/path.rs"));
        assert!(header.contains("new/path.rs"));
        assert!(header.contains("⟶"));
        assert!(header.contains("renamed:"));
    }

    #[test]
    fn diff_line_uses_erase_eol_when_ansi_mode() {
        let theme = Theme::default();
        let line = render_diff_line(
            &LineKind::Added,
            "let x = 1;",
            "test.rs",
            80,
            BgFill::AnsiErase,
            &theme,
        );
        assert!(line.contains("\x1b[0K"), "should contain ERASE_EOL");
    }

    #[test]
    fn diff_line_uses_spaces_when_space_mode() {
        let theme = Theme::default();
        let line = render_diff_line(
            &LineKind::Added,
            "short",
            "test.rs",
            20,
            BgFill::Spaces,
            &theme,
        );
        // "short" = 5 chars, width = 20, so 15 spaces padding
        assert!(!line.contains("\x1b[0K"), "should NOT contain ERASE_EOL");
        // Count trailing spaces before RESET
        let before_reset = line.strip_suffix("\x1b[0m").unwrap();
        assert!(
            before_reset.ends_with("               "),
            "should have 15 trailing spaces"
        );
    }
}
