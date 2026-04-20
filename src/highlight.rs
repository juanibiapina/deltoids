use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect_assets::assets::HighlightingAssets;
use unicode_width::UnicodeWidthChar;

const TAB_WIDTH: usize = 4;
const THEME_NAME: &str = "OneHalfDark";

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

fn to_ratatui_style(base_style: Style, style: syntect::highlighting::Style) -> Style {
    let mut result = base_style.fg(ratatui::style::Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        result = result.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        result = result.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        result = result.add_modifier(Modifier::UNDERLINED);
    }

    result
}

fn push_styled_text(
    spans: &mut Vec<Span<'static>>,
    buffer: &mut String,
    current_style: &mut Option<Style>,
    style: Style,
    text: &str,
) {
    if text.is_empty() {
        return;
    }

    if current_style.is_some_and(|current| current == style) {
        buffer.push_str(text);
        return;
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(
            std::mem::take(buffer),
            current_style.expect("style should exist"),
        ));
    }

    *current_style = Some(style);
    buffer.push_str(text);
}

fn flush_styled_text(
    spans: &mut Vec<Span<'static>>,
    buffer: &mut String,
    current_style: &mut Option<Style>,
) {
    if !buffer.is_empty() {
        spans.push(Span::styled(
            std::mem::take(buffer),
            current_style.expect("style should exist"),
        ));
    }
    *current_style = None;
}

fn truncate_highlighted_ranges(
    ranges: Vec<(syntect::highlighting::Style, &str)>,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut current_style = None;
    let mut width = 0;

    'outer: for (style, segment) in ranges {
        let style = to_ratatui_style(base_style, style);
        for ch in segment.chars() {
            let (text, ch_width) = if ch == '\t' {
                ("    ", TAB_WIDTH)
            } else {
                let ch_width = ch.width().unwrap_or(0);
                if ch_width == 0 {
                    continue;
                }
                let mut text = String::new();
                text.push(ch);
                push_styled_text(&mut spans, &mut buffer, &mut current_style, style, &text);
                width += ch_width;
                if width >= max_width {
                    break 'outer;
                }
                continue;
            };

            if width + ch_width > max_width {
                break 'outer;
            }

            push_styled_text(&mut spans, &mut buffer, &mut current_style, style, text);
            width += ch_width;
        }
    }

    flush_styled_text(&mut spans, &mut buffer, &mut current_style);
    (spans, width)
}

fn plain_text_spans(
    line: &str,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut width = 0;

    for ch in line.chars() {
        let ch_width = if ch == '\t' {
            TAB_WIDTH
        } else {
            ch.width().unwrap_or(0)
        };
        if ch_width == 0 {
            continue;
        }
        if width + ch_width > max_width {
            break;
        }
        if ch == '\t' {
            text.push_str("    ");
        } else {
            text.push(ch);
        }
        width += ch_width;
    }

    spans.push(Span::styled(text, base_style));
    (spans, width)
}

pub(crate) fn highlighted_spans(
    path: &str,
    line: &str,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let syntax = syntax_for_path(path);
    let mut highlighter = HighlightLines::new(syntax, theme());

    match highlighter.highlight_line(line, syntax_set()) {
        Ok(ranges) => truncate_highlighted_ranges(ranges, base_style, max_width),
        Err(_) => plain_text_spans(line, base_style, max_width),
    }
}

use crate::intraline::EmphSection;

/// Produce syntax-highlighted spans with per-section background colors.
///
/// Each `EmphSection` maps to a substring of `line`. The `bg_for_section`
/// callback returns the background color for that section. Syntax foreground
/// colors are preserved.
pub(crate) fn highlighted_spans_with_emphasis(
    path: &str,
    line: &str,
    sections: &[EmphSection],
    bg_for_section: impl Fn(&EmphSection) -> ratatui::style::Color,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let syntax = syntax_for_path(path);
    let mut highlighter = HighlightLines::new(syntax, theme());

    // Build a byte-offset to section-index map.
    let section_ranges = build_section_byte_ranges(sections);

    match highlighter.highlight_line(line, syntax_set()) {
        Ok(ranges) => truncate_with_emphasis(
            ranges,
            sections,
            &section_ranges,
            &bg_for_section,
            max_width,
        ),
        Err(_) => {
            plain_text_with_emphasis(line, sections, &section_ranges, &bg_for_section, max_width)
        }
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
    for (i, &(start, end)) in ranges.iter().enumerate() {
        if byte_offset >= start && byte_offset < end {
            return Some(i);
        }
    }
    None
}

fn truncate_with_emphasis(
    ranges: Vec<(syntect::highlighting::Style, &str)>,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    bg_for_section: &impl Fn(&EmphSection) -> ratatui::style::Color,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut current_style: Option<Style> = None;
    let mut width = 0;
    let mut byte_offset = 0usize;

    'outer: for (syn_style, segment) in ranges {
        for ch in segment.chars() {
            let ch_byte_len = ch.len_utf8();
            let (text, ch_width) = if ch == '\t' {
                ("    ", TAB_WIDTH)
            } else {
                let w = ch.width().unwrap_or(0);
                if w == 0 {
                    byte_offset += ch_byte_len;
                    continue;
                }
                let mut t = String::new();
                t.push(ch);
                // Determine background from emphasis section.
                let bg = section_index_at(byte_offset, section_ranges)
                    .map(|i| bg_for_section(&sections[i]))
                    .unwrap_or(ratatui::style::Color::Reset);
                let style = to_ratatui_style(Style::default().bg(bg), syn_style);
                push_styled_text(&mut spans, &mut buffer, &mut current_style, style, &t);
                width += w;
                byte_offset += ch_byte_len;
                if width >= max_width {
                    break 'outer;
                }
                continue;
            };

            if width + ch_width > max_width {
                break 'outer;
            }

            let bg = section_index_at(byte_offset, section_ranges)
                .map(|i| bg_for_section(&sections[i]))
                .unwrap_or(ratatui::style::Color::Reset);
            let style = to_ratatui_style(Style::default().bg(bg), syn_style);
            push_styled_text(&mut spans, &mut buffer, &mut current_style, style, text);
            width += ch_width;
            byte_offset += ch_byte_len;
        }
    }

    flush_styled_text(&mut spans, &mut buffer, &mut current_style);
    (spans, width)
}

fn plain_text_with_emphasis(
    line: &str,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    bg_for_section: &impl Fn(&EmphSection) -> ratatui::style::Color,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut current_style: Option<Style> = None;
    let mut width = 0;
    let mut byte_offset = 0usize;

    for ch in line.chars() {
        let ch_byte_len = ch.len_utf8();
        let ch_width = if ch == '\t' {
            TAB_WIDTH
        } else {
            ch.width().unwrap_or(0)
        };
        if ch_width == 0 {
            byte_offset += ch_byte_len;
            continue;
        }
        if width + ch_width > max_width {
            break;
        }
        let bg = section_index_at(byte_offset, section_ranges)
            .map(|i| bg_for_section(&sections[i]))
            .unwrap_or(ratatui::style::Color::Reset);
        let style = Style::default().bg(bg);
        let text = if ch == '\t' {
            "    ".to_string()
        } else {
            ch.to_string()
        };
        push_styled_text(&mut spans, &mut buffer, &mut current_style, style, &text);
        width += ch_width;
        byte_offset += ch_byte_len;
    }

    flush_styled_text(&mut spans, &mut buffer, &mut current_style);
    (spans, width)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn detects_rust_syntax_from_path() {
        assert_eq!(syntax_for_path("src/main.rs").name, "Rust");
    }

    #[test]
    fn falls_back_to_plain_text_for_unknown_extension() {
        assert_eq!(syntax_for_path("notes/file.unknown").name, "Plain Text");
    }

    #[test]
    fn highlighted_spans_use_syntax_colors() {
        let (spans, _) = highlighted_spans("src/main.rs", "fn main() {}", Style::default(), 20);

        assert!(spans.iter().any(|span| span.style.fg != Some(Color::Reset)));
    }

    #[test]
    fn highlighted_spans_preserve_base_background() {
        let base_style = Style::default().bg(Color::Rgb(29, 43, 52));
        let (spans, _) = highlighted_spans("src/main.rs", "fn main() {}", base_style, 20);

        assert!(!spans.is_empty());
        assert!(spans.iter().all(|span| span.style.bg == base_style.bg));
    }
}
