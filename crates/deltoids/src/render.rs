//! Render diff output with ANSI colors and breadcrumb boxes.
//!
//! Public entry points are `render_file_header`, `render_rename_header`, and
//! `render_hunk`. `render_hunk` takes the diff's already-detected
//! [`Language`] (typically obtained via `Diff::language()`) so highlighting
//! works for files whose language was resolved via shebang or filename rather
//! than extension.

use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use unicode_width::UnicodeWidthStr;

use crate::config::{SyntaxAssets, Theme, rgb_to_ansi_bg, rgb_to_ansi_fg};
use crate::intraline::{EmphKind, LineEmphasis, compute_subhunk_emphasis};
use crate::{Hunk, HunkRun, Language, LineKind, ScopeNode};

const TAB_WIDTH: usize = 4;

// ANSI control sequences.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ERASE_EOL: &str = "\x1b[0K";
const DEFAULT_FG: &str = "\x1b[38;2;220;223;228m";

/// How to fill background color to end of line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgFill {
    /// Use ANSI CSI sequence (`\x1b[0K`) — efficient but not supported by
    /// `less -R`.
    AnsiErase,
    /// Pad with spaces to terminal width — works through pagers.
    Spaces,
}

/// Render a file header (path line plus separator rule).
pub fn render_file_header(path: &str, width: usize, theme: &Theme) -> Vec<String> {
    let separator_fg = rgb_to_ansi_fg(theme.separator.0, theme.separator.1, theme.separator.2);
    vec![
        format!("{BOLD}{path}{RESET}"),
        format!("{separator_fg}{}{RESET}", "─".repeat(width)),
    ]
}

/// Render a rename header showing `old ⟶ new`.
pub fn render_rename_header(old_path: &str, new_path: &str, theme: &Theme) -> String {
    let muted_fg = rgb_to_ansi_fg(theme.muted.0, theme.muted.1, theme.muted.2);
    format!("{muted_fg}renamed: {old_path} ⟶ {new_path}{RESET}")
}

/// Render a full hunk: breadcrumb box (if any ancestors) followed by the
/// diff body with intraline emphasis. Highlighting uses `language`, which
/// callers should obtain from `Diff::language()`.
pub fn render_hunk(
    hunk: &Hunk,
    language: Option<Language>,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Vec<String> {
    let mut output = Vec::new();

    if !hunk.ancestors.is_empty() {
        output.extend(render_breadcrumb_box(
            &hunk.ancestors,
            language,
            width,
            theme,
        ));
    }

    for run in hunk.runs() {
        match run {
            HunkRun::Context(line) => {
                output.push(render_diff_line(
                    &line.kind,
                    &line.content,
                    language,
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
                output.extend(render_subhunk(&subhunk, language, width, fill, theme));
            }
        }
    }

    output
}

// ---------------------------------------------------------------------------
// Internal renderers
// ---------------------------------------------------------------------------

/// Render the breadcrumb box for a hunk's enclosing scopes.
fn render_breadcrumb_box(
    ancestors: &[ScopeNode],
    language: Option<Language>,
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

    let max_line_num = ancestors.iter().map(|a| a.start_line).max().unwrap_or(0);
    let num_col_width = max_line_num.to_string().len();

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
                let highlighted = highlight_line(text, language);
                let text_width = display_width(text);
                let padding = content_width.saturating_sub(prefix_width + text_width);

                lines.push(format!(
                    "{line_num_fg}{num_str}{RESET}{highlighted}{}{border_fg} │{RESET}",
                    " ".repeat(padding)
                ));
            }
            None => {
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

/// Render one diff line (added/removed/context) with syntax highlighting and
/// the appropriate background.
fn render_diff_line(
    kind: &LineKind,
    content: &str,
    language: Option<Language>,
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

    let highlighted = highlight_line(content, language);

    if bg.is_empty() {
        format!("{highlighted}{RESET}")
    } else {
        let fill_str = bg_fill_string(content, width, fill);
        if highlighted.is_empty() {
            format!("{bg}{DEFAULT_FG}{fill_str}{RESET}")
        } else {
            format!("{bg}{highlighted}{fill_str}{RESET}")
        }
    }
}

/// Render a diff line with intraline emphasis (paired added/removed runs).
fn render_diff_line_with_emphasis(
    kind: &LineKind,
    content: &str,
    emphasis: &LineEmphasis,
    language: Option<Language>,
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
        LineKind::Context => {
            return render_diff_line(kind, content, language, width, fill, theme);
        }
    };

    match emphasis {
        LineEmphasis::Plain => render_diff_line(kind, content, language, width, fill, theme),
        LineEmphasis::Paired(sections) => {
            let mut result = String::new();
            result.push_str(&plain_bg);

            for section in sections {
                let bg = match section.kind {
                    EmphKind::Emph => &emph_bg,
                    EmphKind::NonEmph => &plain_bg,
                };
                let highlighted = highlight_line(&section.text, language);
                result.push_str(bg);
                result.push_str(&highlighted);
            }

            result.push_str(&plain_bg);
            if content.is_empty() {
                result.push_str(DEFAULT_FG);
            }
            result.push_str(&bg_fill_string(content, width, fill));
            result.push_str(RESET);
            result
        }
    }
}

/// Render a run of consecutive +/- lines with intraline emphasis.
fn render_subhunk(
    lines: &[(LineKind, &str)],
    language: Option<Language>,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Vec<String> {
    let mut minus_lines: Vec<&str> = Vec::new();
    let mut plus_lines: Vec<&str> = Vec::new();

    for (kind, content) in lines {
        match kind {
            LineKind::Removed => minus_lines.push(content),
            LineKind::Added => plus_lines.push(content),
            LineKind::Context => {}
        }
    }

    let (minus_emphasis, plus_emphasis) = compute_subhunk_emphasis(&minus_lines, &plus_lines);

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
                    language,
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
                    language,
                    width,
                    fill,
                    theme,
                ));
                plus_idx += 1;
            }
            LineKind::Context => {
                output.push(render_diff_line(
                    kind, content, language, width, fill, theme,
                ));
            }
        }
    }

    output
}

/// Syntax-highlight a line and return ANSI-escaped string.
///
/// Sets foreground colors only — the caller owns background. This lets
/// background colors persist across all tokens.
fn highlight_line(line: &str, language: Option<Language>) -> String {
    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for(language);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => {
            let mut result = String::new();
            for (style, text) in ranges {
                let text = text.replace('\t', &" ".repeat(TAB_WIDTH));
                let fg = syntect_color_to_ansi_fg(style.foreground);

                if style.font_style.contains(FontStyle::BOLD) {
                    result.push_str(BOLD);
                }
                result.push_str(&fg);
                result.push_str(&text);
            }
            result
        }
        Err(_) => line.replace('\t', &" ".repeat(TAB_WIDTH)),
    }
}

/// Convert a syntect color to an ANSI foreground escape sequence.
///
/// The "ansi" theme encodes ANSI color indices specially:
/// - `r=N, g=0, b=0, a=0` means ANSI color N (0-15)
/// - `r=0, g=0, b=0, a=1` means default foreground
fn syntect_color_to_ansi_fg(color: syntect::highlighting::Color) -> String {
    if color.g == 0 && color.b == 0 && color.a == 0 && color.r <= 15 {
        return format!("\x1b[38;5;{}m", color.r);
    }
    if color.r == 0 && color.g == 0 && color.b == 0 && color.a == 1 {
        return "\x1b[39m".to_string();
    }
    format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diff, DiffLine};

    #[test]
    fn file_header_contains_path() {
        let theme = Theme::default();
        let header = render_file_header("src/main.rs", 80, &theme);
        assert_eq!(header.len(), 2);
        assert!(header[0].contains("src/main.rs"));
        assert!(header[1].contains("───"));
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

    fn rust_function_hunk() -> Hunk {
        Hunk {
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
        }
    }

    #[test]
    fn render_hunk_includes_breadcrumb_when_ancestors_present() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some(Language::Rust), 80, BgFill::AnsiErase, &theme);
        assert!(lines.iter().any(|l| l.contains("┐")));
    }

    #[test]
    fn render_hunk_uses_language_for_highlighting_extensionless_path() {
        // The bug this guards against: `Diff::compute(path="script", ...)`
        // detects Bash from the shebang, but a renderer that ignores the
        // detected language and re-detects per line falls back to plain text
        // because no single line carries the shebang.
        let original = "#!/usr/bin/env bash\n\nfunction name() {\n  echo old\n}\n";
        let updated = "#!/usr/bin/env bash\n\nfunction name() {\n  echo new\n}\n";

        let with_extension = Diff::compute(original, updated, "script.sh");
        let extensionless = Diff::compute(original, updated, "script");

        assert_eq!(with_extension.language(), Some(Language::Bash));
        assert_eq!(extensionless.language(), Some(Language::Bash));

        let theme = Theme::default();
        let render = |diff: &Diff| -> Vec<String> {
            diff.hunks()
                .iter()
                .flat_map(|h| render_hunk(h, diff.language(), 120, BgFill::Spaces, &theme))
                .collect()
        };

        assert_eq!(render(&with_extension), render(&extensionless));
    }

    #[test]
    fn render_hunk_added_line_carries_added_bg() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some(Language::Rust), 80, BgFill::AnsiErase, &theme);
        assert!(lines.iter().any(|l| l.contains("\x1b[48;2;32;48;59m")));
    }

    #[test]
    fn render_hunk_paired_change_uses_emphasis_bg() {
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Removed,
                    content: "const x = 1;".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "const x = 2;".to_string(),
                },
            ],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, Some(Language::Rust), 80, BgFill::AnsiErase, &theme);
        // No ancestors -> no breadcrumb box; rendered lines are the change
        // run in order: removed first, added second.
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\x1b[48;2;113;49;55m"), "minus emph bg");
        assert!(lines[1].contains("\x1b[48;2;44;90;102m"), "plus emph bg");
    }

    #[test]
    fn render_hunk_dissimilar_change_skips_emphasis_bg() {
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Removed,
                    content: "aaa bbb ccc ddd eee fff ggg hhh".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "xxx yyy zzz www uuu vvv ppp qqq".to_string(),
                },
            ],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, Some(Language::Rust), 80, BgFill::AnsiErase, &theme);
        for line in &lines {
            assert!(
                !line.contains("\x1b[48;2;113;49;55m"),
                "minus should not have emph bg: {line}"
            );
            assert!(
                !line.contains("\x1b[48;2;44;90;102m"),
                "plus should not have emph bg: {line}"
            );
        }
    }

    #[test]
    fn render_hunk_uses_erase_eol_in_ansi_mode() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some(Language::Rust), 80, BgFill::AnsiErase, &theme);
        assert!(lines.iter().any(|l| l.contains("\x1b[0K")));
    }

    #[test]
    fn render_hunk_uses_space_padding_when_requested() {
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "short".to_string(),
            }],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, Some(Language::Rust), 20, BgFill::Spaces, &theme);
        let line = lines.first().expect("rendered line");
        assert!(!line.contains("\x1b[0K"));
        let before_reset = line.strip_suffix("\x1b[0m").expect("reset suffix");
        assert!(
            before_reset.ends_with("               "),
            "expected 15 trailing spaces, got {line:?}"
        );
    }
}
