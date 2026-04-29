use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use unicode_width::UnicodeWidthChar;

use deltoids::Language;

use crate::theme::ResolvedTheme;

const TAB_WIDTH: usize = 4;

/// Convert syntect foreground color to ratatui Color.
///
/// The "ansi" theme encodes ANSI color indices specially:
/// - `r=N, g=0, b=0, a=0` means ANSI color N (0-15)
/// - `r=0, g=0, b=0, a=1` means default foreground
fn syntect_to_ratatui_color(color: syntect::highlighting::Color) -> ratatui::style::Color {
    use ratatui::style::Color;

    // ANSI theme encoding: g=0, b=0, a=0 means r is the ANSI color index
    if color.g == 0 && color.b == 0 && color.a == 0 && color.r <= 15 {
        return Color::Indexed(color.r);
    }

    // Default foreground (a=1 with black RGB)
    if color.r == 0 && color.g == 0 && color.b == 0 && color.a == 1 {
        return Color::Reset;
    }

    // Actual RGB color
    Color::Rgb(color.r, color.g, color.b)
}

fn to_ratatui_style(base_style: Style, style: syntect::highlighting::Style) -> Style {
    let mut result = base_style.fg(syntect_to_ratatui_color(style.foreground));

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

struct VisibleChar {
    text: String,
    width: usize,
    byte_len: usize,
}

fn visible_char(ch: char) -> Option<VisibleChar> {
    if ch == '\t' {
        return Some(VisibleChar {
            text: "    ".to_string(),
            width: TAB_WIDTH,
            byte_len: ch.len_utf8(),
        });
    }

    let width = ch.width().unwrap_or(0);
    if width == 0 {
        return None;
    }

    Some(VisibleChar {
        text: ch.to_string(),
        width,
        byte_len: ch.len_utf8(),
    })
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
            let Some(visible) = visible_char(ch) else {
                continue;
            };

            if width + visible.width > max_width {
                break 'outer;
            }

            push_styled_text(
                &mut spans,
                &mut buffer,
                &mut current_style,
                style,
                &visible.text,
            );
            width += visible.width;
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
        let Some(visible) = visible_char(ch) else {
            continue;
        };
        if width + visible.width > max_width {
            break;
        }
        text.push_str(&visible.text);
        width += visible.width;
    }

    spans.push(Span::styled(text, base_style));
    (spans, width)
}

pub(crate) fn highlighted_spans(
    theme: &ResolvedTheme,
    language: Option<Language>,
    line: &str,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let syntax = theme.syntax_assets.syntax_for(language);
    let mut highlighter = HighlightLines::new(syntax, theme.syntax_assets.syntax_theme);

    match highlighter.highlight_line(line, theme.syntax_assets.syntax_set) {
        Ok(ranges) => truncate_highlighted_ranges(ranges, base_style, max_width),
        Err(_) => plain_text_spans(line, base_style, max_width),
    }
}

use deltoids::EmphSection;

/// Produce syntax-highlighted spans with per-section background colors.
///
/// Each `EmphSection` maps to a substring of `line`. The `bg_for_section`
/// callback returns the background color for that section. Syntax foreground
/// colors are preserved.
pub(crate) fn highlighted_spans_with_emphasis(
    theme: &ResolvedTheme,
    language: Option<Language>,
    line: &str,
    sections: &[EmphSection],
    bg_for_section: impl Fn(&EmphSection) -> ratatui::style::Color,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let syntax = theme.syntax_assets.syntax_for(language);
    let mut highlighter = HighlightLines::new(syntax, theme.syntax_assets.syntax_theme);

    // Build a byte-offset to section-index map.
    let section_ranges = build_section_byte_ranges(sections);

    match highlighter.highlight_line(line, theme.syntax_assets.syntax_set) {
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
            let Some(visible) = visible_char(ch) else {
                byte_offset += ch.len_utf8();
                continue;
            };

            if width + visible.width > max_width {
                break 'outer;
            }

            let bg = section_index_at(byte_offset, section_ranges)
                .map(|i| bg_for_section(&sections[i]))
                .unwrap_or(ratatui::style::Color::Reset);
            let style = to_ratatui_style(Style::default().bg(bg), syn_style);
            push_styled_text(
                &mut spans,
                &mut buffer,
                &mut current_style,
                style,
                &visible.text,
            );
            width += visible.width;
            byte_offset += visible.byte_len;
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
        let Some(visible) = visible_char(ch) else {
            byte_offset += ch.len_utf8();
            continue;
        };
        if width + visible.width > max_width {
            break;
        }
        let bg = section_index_at(byte_offset, section_ranges)
            .map(|i| bg_for_section(&sections[i]))
            .unwrap_or(ratatui::style::Color::Reset);
        let style = Style::default().bg(bg);
        push_styled_text(
            &mut spans,
            &mut buffer,
            &mut current_style,
            style,
            &visible.text,
        );
        width += visible.width;
        byte_offset += visible.byte_len;
    }

    flush_styled_text(&mut spans, &mut buffer, &mut current_style);
    (spans, width)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    fn test_theme() -> ResolvedTheme {
        ResolvedTheme::resolve()
    }

    #[test]
    fn highlighted_spans_use_syntax_colors_for_supported_language() {
        let theme = test_theme();
        let (spans, _) = highlighted_spans(
            &theme,
            Some(Language::Rust),
            "fn main() {}",
            Style::default(),
            20,
        );

        assert!(spans.iter().any(|span| span.style.fg != Some(Color::Reset)));
    }

    #[test]
    fn highlighted_spans_falls_back_to_plain_when_language_unknown() {
        let theme = test_theme();
        let (spans, _) = highlighted_spans(&theme, None, "fn main() {}", Style::default(), 20);

        // Plain text means a single base-styled span.
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn highlighted_spans_preserve_base_background() {
        let theme = test_theme();
        let base_style = Style::default().bg(Color::Rgb(29, 43, 52));
        let (spans, _) =
            highlighted_spans(&theme, Some(Language::Rust), "fn main() {}", base_style, 20);

        assert!(!spans.is_empty());
        assert!(spans.iter().all(|span| span.style.bg == base_style.bg));
    }

    #[test]
    fn visible_char_expands_tabs() {
        let visible = visible_char('\t').expect("tab should be visible");
        assert_eq!(visible.text, "    ");
        assert_eq!(visible.width, 4);
        assert_eq!(visible.byte_len, 1);
    }
}
