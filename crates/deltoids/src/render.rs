//! Render diff output with ANSI colors and breadcrumb boxes.
//!
//! Public entry points are `render_file_header`, `render_rename_header`, and
//! `render_hunk`. `render_hunk` takes the diff's already-detected highlight
//! syntax name (obtained via `Diff::highlight()`) so highlighting works for
//! files whose syntax was resolved via shebang or filename rather
//! than extension.

use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;

use crate::config::{SyntaxAssets, Theme, rgb_to_ansi_bg, rgb_to_ansi_fg};
use crate::hunk_header::{Breadcrumb, BreadcrumbRow, HunkHeader, display_width};
use crate::intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
use crate::{Hunk, HunkRun, LineKind};

const TAB_WIDTH: usize = 4;

// ANSI control sequences.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
/// Clears only the bold/italic/underline attributes (22/23/24), leaving
/// foreground and background intact.
const FONT_STYLE_RESET: &str = "\x1b[22;23;24m";
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
/// diff body with intraline emphasis. Highlighting uses `highlight` (a
/// syntect syntax name), which callers should obtain from `Diff::highlight()`.
pub fn render_hunk(
    hunk: &Hunk,
    highlight: Option<&str>,
    width: usize,
    fill: BgFill,
    theme: &Theme,
) -> Vec<String> {
    let mut output = Vec::new();

    match HunkHeader::plan(hunk, width) {
        HunkHeader::LineNumber { line_num } => {
            output.extend(render_line_number_box(line_num, theme));
        }
        HunkHeader::Breadcrumb(b) => {
            output.extend(render_breadcrumb_box(&b, highlight, theme));
        }
    }

    for run in hunk.runs() {
        match run {
            HunkRun::Context(line) => {
                output.push(render_diff_line(
                    &line.kind,
                    &line.content,
                    highlight,
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
                output.extend(render_subhunk(&subhunk, highlight, width, fill, theme));
            }
        }
    }

    output
}

// ---------------------------------------------------------------------------
// Internal renderers
// ---------------------------------------------------------------------------

/// Render a three-line box containing only the new-file line number. Used
/// when a hunk has no enclosing structural scope, mirroring the ratatui
/// renderer with sharp corners.
fn render_line_number_box(line_num: usize, theme: &Theme) -> Vec<String> {
    let border_fg = rgb_to_ansi_fg(theme.border.0, theme.border.1, theme.border.2);
    let label = line_num.to_string();
    let inner = label.len() + 1;
    vec![
        format!("{border_fg}{}┐{RESET}", "─".repeat(inner)),
        format!("{border_fg}{label} │{RESET}"),
        format!("{border_fg}{}┘{RESET}", "─".repeat(inner)),
    ]
}

/// Paint the breadcrumb box from a shared [`Breadcrumb`] plan. Geometry comes
/// from the plan; this function only paints ANSI strings (sharp corners, no
/// truncation).
fn render_breadcrumb_box(b: &Breadcrumb, highlight: Option<&str>, theme: &Theme) -> Vec<String> {
    let border_fg = rgb_to_ansi_fg(theme.border.0, theme.border.1, theme.border.2);
    let line_num_fg = rgb_to_ansi_fg(
        theme.line_number.0,
        theme.line_number.1,
        theme.line_number.2,
    );

    let num_col_width = b.num_col_width;
    let prefix_width = b.prefix_width();
    let content_width = b.content_width;

    let top = format!("{border_fg}{}┐{RESET}", "─".repeat(content_width + 1));
    let bot = format!("{border_fg}{}┘{RESET}", "─".repeat(content_width + 1));

    let mut lines = vec![top];

    for row in &b.rows {
        match row {
            BreadcrumbRow::Scope { line_num, text } => {
                let num_str = format!("{line_num:>num_col_width$}: ");
                let highlighted = highlight_line(text, highlight);
                let text_width = display_width(text);
                let padding = content_width.saturating_sub(prefix_width + text_width);

                lines.push(format!(
                    "{line_num_fg}{num_str}{RESET}{highlighted}{}{border_fg} │{RESET}",
                    " ".repeat(padding)
                ));
            }
            BreadcrumbRow::Gap => {
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
    highlight: Option<&str>,
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

    let highlighted = highlight_line(content, highlight);

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
    highlight: Option<&str>,
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
            return render_diff_line(kind, content, highlight, width, fill, theme);
        }
    };

    match emphasis {
        LineEmphasis::Plain => render_diff_line(kind, content, highlight, width, fill, theme),
        LineEmphasis::Paired(sections) => {
            let section_ranges = build_section_byte_ranges(sections);
            let body = highlight_line_emphasized(
                content,
                highlight,
                sections,
                &section_ranges,
                &plain_bg,
                &emph_bg,
            );

            let mut result = String::new();
            result.push_str(&plain_bg);
            result.push_str(&body);
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
    highlight: Option<&str>,
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
                    highlight,
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
                    highlight,
                    width,
                    fill,
                    theme,
                ));
                plus_idx += 1;
            }
            LineKind::Context => {
                output.push(render_diff_line(
                    kind, content, highlight, width, fill, theme,
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
///
/// Limitation: each line is highlighted with fresh syntect state, so
/// context-dependent tokens in stateful grammars lose color (e.g. a
/// Dockerfile `RUN`/`ENV` keyword is only colored once the parser has seen
/// the preceding `FROM`). Carrying state across hunk lines would fix this.
fn highlight_line(line: &str, highlight: Option<&str>) -> String {
    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => {
            let mut result = String::new();
            for (style, text) in ranges {
                let text = text.replace('\t', &" ".repeat(TAB_WIDTH));
                let fg = syntect_color_to_ansi_fg(style.foreground);

                result.push_str(&font_style_sgr(style.font_style));
                result.push_str(&fg);
                result.push_str(&text);
            }
            // Clear the three font-style attributes so a trailing italic/bold/
            // underline token cannot bleed into whatever the caller appends
            // next (breadcrumb border, background padding). Not a full reset:
            // callers own the background.
            result.push_str(FONT_STYLE_RESET);
            result
        }
        Err(_) => line.replace('\t', &" ".repeat(TAB_WIDTH)),
    }
}

/// Map a syntect [`FontStyle`] to an SGR attribute sequence, emitting an
/// explicit on/off code for each of bold, italic, and underline so no
/// attribute bleeds across tokens. The off-codes (22/23/24) clear only their
/// own attribute and leave foreground (3x) and background (4x) untouched.
fn font_style_sgr(font_style: FontStyle) -> String {
    let bold = if font_style.contains(FontStyle::BOLD) {
        "1"
    } else {
        "22"
    };
    let italic = if font_style.contains(FontStyle::ITALIC) {
        "3"
    } else {
        "23"
    };
    let underline = if font_style.contains(FontStyle::UNDERLINE) {
        "4"
    } else {
        "24"
    };
    format!("\x1b[{bold};{italic};{underline}m")
}

/// Highlight the whole line once, then overlay per-character emphasis
/// backgrounds.
///
/// Highlighting the entire line (rather than each emphasis section on its own)
/// keeps multi-token scopes like line comments and strings intact: a section
/// starting mid-comment would otherwise be re-tokenized from scratch, losing
/// the enclosing comment scope and coloring identifiers as code. This mirrors
/// the ratatui renderer, where emphasis only changes the background.
fn highlight_line_emphasized(
    content: &str,
    highlight: Option<&str>,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    plain_bg: &str,
    emph_bg: &str,
) -> String {
    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    let mut result = String::new();
    let mut byte_offset = 0usize;
    match highlighter.highlight_line(content, assets.syntax_set) {
        Ok(ranges) => {
            for (style, text) in ranges {
                let fg = syntect_color_to_ansi_fg(style.foreground);
                let sgr = font_style_sgr(style.font_style);
                emit_emphasized_segment(
                    &mut result,
                    text,
                    &mut byte_offset,
                    &sgr,
                    &fg,
                    sections,
                    section_ranges,
                    plain_bg,
                    emph_bg,
                );
            }
            // Clear font-style attributes so a trailing styled token cannot
            // bleed into the caller's background padding.
            result.push_str(FONT_STYLE_RESET);
        }
        Err(_) => {
            emit_emphasized_segment(
                &mut result,
                content,
                &mut byte_offset,
                "",
                "",
                sections,
                section_ranges,
                plain_bg,
                emph_bg,
            );
        }
    }
    result
}

/// Emit one syntect segment with per-character emphasis backgrounds, coalescing
/// consecutive characters that share a background into one run. `sgr` and `fg`
/// are constant for the segment; only the background varies across emphasis
/// section boundaries. Advances `byte_offset` by each character's source byte
/// length (tabs expand to spaces in the output but count as one source byte).
#[allow(clippy::too_many_arguments)]
fn emit_emphasized_segment(
    out: &mut String,
    segment: &str,
    byte_offset: &mut usize,
    sgr: &str,
    fg: &str,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    plain_bg: &str,
    emph_bg: &str,
) {
    let bg_at = |offset: usize| -> &str {
        match section_index_at(offset, section_ranges) {
            Some(i) if matches!(sections[i].kind, EmphKind::Emph) => emph_bg,
            _ => plain_bg,
        }
    };

    let mut current_bg: Option<&str> = None;
    for ch in segment.chars() {
        let bg = bg_at(*byte_offset);
        if current_bg != Some(bg) {
            out.push_str(bg);
            out.push_str(sgr);
            out.push_str(fg);
            current_bg = Some(bg);
        }
        if ch == '\t' {
            out.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            out.push(ch);
        }
        *byte_offset += ch.len_utf8();
    }
}

/// For each emphasis section, compute its byte range in the original line.
fn build_section_byte_ranges(sections: &[EmphSection]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::with_capacity(sections.len());
    let mut offset = 0;
    for section in sections {
        let start = offset;
        let end = start + section.text.len();
        ranges.push((start, end));
        offset = end;
    }
    ranges
}

/// Find which emphasis section a byte offset falls in.
fn section_index_at(byte_offset: usize, ranges: &[(usize, usize)]) -> Option<usize> {
    ranges
        .iter()
        .position(|&(start, end)| byte_offset >= start && byte_offset < end)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diff, DiffLine, Language, ScopeNode};

    /// Collect foreground color escapes (`38;2;...`, `38;5;...`, `39`) in
    /// order, ignoring background and attribute codes.
    fn fg_codes(s: &str) -> Vec<String> {
        // Each SGR sequence is `ESC [ <codes> m`; splitting on `ESC [` yields
        // `<codes>m<text>` parts, so the code runs up to the first `m`.
        s.split('\x1b')
            .filter_map(|part| part.strip_prefix('['))
            .filter_map(|part| part.split_once('m').map(|(seq, _)| seq))
            .filter(|seq| seq.starts_with("38;") || *seq == "39")
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn emphasis_does_not_fragment_syntax_highlighting() {
        // A changed comment line gets intraline emphasis. The emphasis must
        // only change the background; it must never re-tokenize a mid-line
        // section on its own (which would drop the enclosing comment scope and
        // color `identifiers` as code). The set of syntax foreground colors
        // applied to an emphasized line must equal those applied to the same
        // line highlighted whole as context.
        let theme = Theme::default();
        let content = "// foo `bar` baz";
        let sections = vec![
            EmphSection {
                kind: EmphKind::NonEmph,
                text: "// foo ".to_string(),
            },
            EmphSection {
                kind: EmphKind::Emph,
                text: "`bar`".to_string(),
            },
            EmphSection {
                kind: EmphKind::NonEmph,
                text: " baz".to_string(),
            },
        ];
        let emph = LineEmphasis::Paired(sections);
        let emphasized = render_diff_line_with_emphasis(
            &LineKind::Added,
            content,
            &emph,
            Some("TypeScriptReact"),
            80,
            BgFill::Spaces,
            &theme,
        );
        let context = render_diff_line(
            &LineKind::Context,
            content,
            Some("TypeScriptReact"),
            80,
            BgFill::Spaces,
            &theme,
        );
        let mut e = fg_codes(&emphasized);
        e.sort();
        e.dedup();
        let mut c = fg_codes(&context);
        c.sort();
        c.dedup();
        assert_eq!(e, c, "emphasis changed which syntax colors were applied");
    }

    #[test]
    fn font_style_sgr_maps_all_combinations() {
        // Each attribute emits an explicit on/off code so state never bleeds
        // across tokens: bold 1/22, italic 3/23, underline 4/24.
        assert_eq!(font_style_sgr(FontStyle::empty()), "\x1b[22;23;24m");
        assert_eq!(font_style_sgr(FontStyle::BOLD), "\x1b[1;23;24m");
        assert_eq!(font_style_sgr(FontStyle::ITALIC), "\x1b[22;3;24m");
        assert_eq!(font_style_sgr(FontStyle::UNDERLINE), "\x1b[22;23;4m");
        assert_eq!(
            font_style_sgr(FontStyle::BOLD | FontStyle::ITALIC),
            "\x1b[1;3;24m"
        );
        assert_eq!(
            font_style_sgr(FontStyle::BOLD | FontStyle::UNDERLINE),
            "\x1b[1;23;4m"
        );
        assert_eq!(
            font_style_sgr(FontStyle::ITALIC | FontStyle::UNDERLINE),
            "\x1b[22;3;4m"
        );
        assert_eq!(
            font_style_sgr(FontStyle::BOLD | FontStyle::ITALIC | FontStyle::UNDERLINE),
            "\x1b[1;3;4m"
        );
    }

    #[test]
    fn highlight_line_appends_font_style_reset() {
        // Regardless of theme, the Ok path clears bold/italic/underline at the
        // end so a trailing styled token cannot bleed into the caller's border
        // or background padding.
        let out = highlight_line("let x = 1;", Some("Rust"));
        assert!(
            out.ends_with(FONT_STYLE_RESET),
            "expected trailing font-style reset, got {out:?}"
        );
    }

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
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::AnsiErase, &theme);
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
        assert_eq!(with_extension.highlight(), extensionless.highlight());

        let theme = Theme::default();
        let render = |diff: &Diff| -> Vec<String> {
            diff.hunks()
                .iter()
                .flat_map(|h| render_hunk(h, diff.highlight(), 120, BgFill::Spaces, &theme))
                .collect()
        };

        assert_eq!(render(&with_extension), render(&extensionless));
    }

    #[test]
    fn render_hunk_highlights_dockerfile_without_tree_sitter() {
        // Dockerfile has no tree-sitter `Language` but still highlights.
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![DiffLine {
                kind: LineKind::Context,
                content: "FROM rust:1 AS builder".to_string(),
            }],
            ancestors: Vec::new(),
        };
        let highlighted = render_hunk(&hunk, Some("Dockerfile"), 80, BgFill::Spaces, &theme);
        let plain = render_hunk(&hunk, None, 80, BgFill::Spaces, &theme);
        assert!(
            highlighted.iter().any(|l| l.contains("\x1b[38;2;")),
            "expected a truecolor foreground escape, got {highlighted:?}"
        );
        assert_ne!(highlighted, plain);
    }

    #[test]
    fn render_hunk_added_line_carries_added_bg() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::AnsiErase, &theme);
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::AnsiErase, &theme);
        // No ancestors -> a 3-line line-number box precedes the change run
        // (removed first, added second).
        assert_eq!(lines.len(), 5);
        assert!(lines[3].contains("\x1b[48;2;113;49;55m"), "minus emph bg");
        assert!(lines[4].contains("\x1b[48;2;44;90;102m"), "plus emph bg");
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::AnsiErase, &theme);
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::AnsiErase, &theme);
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
        let lines = render_hunk(&hunk, Some("Rust"), 20, BgFill::Spaces, &theme);
        // The line-number box occupies the first 3 lines; the padded diff
        // line follows.
        let line = &lines[3];
        assert!(!line.contains("\x1b[0K"));
        let before_reset = line.strip_suffix("\x1b[0m").expect("reset suffix");
        assert!(
            before_reset.ends_with("               "),
            "expected 15 trailing spaces, got {line:?}"
        );
    }

    #[test]
    fn render_hunk_emits_line_number_box_when_no_ancestors() {
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 1,
            new_start: 42,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "x".to_string(),
            }],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, Some("Rust"), 80, BgFill::Spaces, &theme);
        let border_fg = rgb_to_ansi_fg(theme.border.0, theme.border.1, theme.border.2);
        assert!(lines[0].contains("┐"), "top box corner");
        assert!(lines[0].contains(&border_fg), "box carries border fg");
        assert!(
            lines[1].contains("42"),
            "box shows the new-file line number"
        );
        assert!(lines[2].contains("┘"), "bottom box corner");
    }
}
