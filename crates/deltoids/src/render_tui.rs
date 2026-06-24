//! Render diff hunks and per-file headers as ratatui [`Line<'static>`] values.
//!
//! Sibling of [`crate::render`] (which emits ANSI strings). Same inputs,
//! same look — different output type for embedding in scrollable
//! [`ratatui::widgets::Paragraph`]s.
//!
//! Available only when the `ratatui` cargo feature is enabled.
//!
//! Public surface:
//!
//! - [`render_hunk`] — breadcrumb / line-number box plus the body of one
//!   hunk (context + intraline-emphasised subhunks).
//! - [`render_file_header`] — bold path line plus separator rule.
//! - [`render_rename_header`] — single muted line `renamed: old ⟶ new`.
//!
//! All three accept `&Theme` for colours and load syntax assets internally
//! via [`SyntaxAssets::load`] (cached). Callers do not pass syntax assets.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::scrollbar as scrollbar_symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use unicode_width::UnicodeWidthChar;

use crate::config::{SyntaxAssets, Theme};
use crate::intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
use crate::{Hunk, HunkRun, LineKind, ScopeNode};

const TAB_WIDTH: usize = 4;

/// Convert a `Theme` RGB tuple to a ratatui [`Color`].
pub fn rgb_to_color(rgb: (u8, u8, u8)) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Render a file header: bold path on line 1, separator rule on line 2.
pub fn render_file_header(path: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let separator = Style::default().fg(rgb_to_color(theme.separator));
    vec![
        Line::from(Span::styled(
            path.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("─".repeat(width), separator)),
    ]
}

/// Render a `renamed: old ⟶ new` line.
pub fn render_rename_header(old_path: &str, new_path: &str, theme: &Theme) -> Line<'static> {
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    Line::from(Span::styled(
        format!("renamed: {old_path} ⟶ {new_path}"),
        muted,
    ))
}

/// Render a full hunk: breadcrumb / line-number box followed by the diff
/// body (context lines + intraline-emphasised subhunks). Highlighting uses
/// `highlight`, which callers should obtain from `Diff::highlight()`.
pub fn render_hunk(
    hunk: &Hunk,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut output = Vec::new();
    output.extend(render_hunk_header(hunk, highlight, width, theme));

    for run in hunk.runs() {
        match run {
            HunkRun::Context(line) => {
                output.push(syntax_diff_line(
                    &line.content,
                    Color::Reset,
                    highlight,
                    width,
                    theme,
                ));
            }
            HunkRun::Change(slice) => {
                render_subhunk(slice, highlight, width, theme, &mut output);
            }
        }
    }

    output
}

// ---------------------------------------------------------------------------
// Hunk header (breadcrumb box or line-number-only box)
// ---------------------------------------------------------------------------

fn render_hunk_header(
    hunk: &Hunk,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if hunk.ancestors.is_empty() {
        render_line_number_box(hunk.new_start, theme)
    } else {
        render_breadcrumb_box(&hunk.ancestors, highlight, width, theme)
    }
}

/// Three-line box containing only the new-file line number. Used when a
/// hunk has no enclosing structural scope.
fn render_line_number_box(line_number: usize, theme: &Theme) -> Vec<Line<'static>> {
    let border = Style::default().fg(rgb_to_color(theme.border));
    let label = line_number.to_string();
    let label_width = display_width(&label);
    let top = format!("{}╮", "─".repeat(label_width + 1));
    let mid = format!("{label} │");
    let bot = format!("{}╯", "─".repeat(label_width + 1));
    vec![
        Line::from(Span::styled(top, border)),
        Line::from(Span::styled(mid, border)),
        Line::from(Span::styled(bot, border)),
    ]
}

fn render_breadcrumb_box(
    ancestors: &[ScopeNode],
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let border = Style::default().fg(rgb_to_color(theme.border));
    let max_content_width = width.saturating_sub(2); // room for " │"

    // Compute the widest line number for right-alignment.
    let max_line_num = ancestors.iter().map(|a| a.start_line).max().unwrap_or(0);
    let num_col_width = max_line_num.to_string().len();

    // Build content rows: ancestor lines with optional "..." gaps between.
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

    // Compute the widest rendered line for box width.
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

    let top = format!("{}╮", "─".repeat(content_width + 1));
    let bot = format!("{}╯", "─".repeat(content_width + 1));

    let mut lines = vec![Line::from(Span::styled(top, border))];

    for row in &rows {
        match &row.text {
            Some(text) => {
                let num_str = format!(
                    "{:>width$}: ",
                    row.line_num.unwrap_or(0),
                    width = num_col_width
                );
                let available_text_width = content_width.saturating_sub(prefix_width);
                let (mut code_spans, code_width) = highlighted_spans(
                    theme,
                    highlight,
                    text,
                    Style::default(),
                    available_text_width.max(1),
                );
                let padding = content_width.saturating_sub(prefix_width + code_width);

                let mut spans = vec![Span::styled(num_str, border)];
                spans.append(&mut code_spans);
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::styled(" │", border));
                lines.push(Line::from(spans));
            }
            None => {
                // "..." gap row
                let dots = format!("{:>width$}  ...", "", width = num_col_width);
                let padding = content_width.saturating_sub(display_width(&dots));
                let mut spans = vec![Span::styled(dots, border)];
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::styled(" │", border));
                lines.push(Line::from(spans));
            }
        }
    }

    lines.push(Line::from(Span::styled(bot, border)));
    lines
}

// ---------------------------------------------------------------------------
// Body: context lines and subhunks (with intraline emphasis)
// ---------------------------------------------------------------------------

/// Render a subhunk (a `Hunk::runs` `Change` slice) with within-line
/// emphasis. The slice is a maximal run of consecutive `Removed`/`Added`
/// lines; pairing the two pools drives intraline emphasis.
fn render_subhunk(
    lines: &[crate::DiffLine],
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
    rendered: &mut Vec<Line<'static>>,
) {
    let minus_contents: Vec<&str> = lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .map(|l| l.content.as_str())
        .collect();
    let plus_contents: Vec<&str> = lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.as_str())
        .collect();

    let (minus_emphasis, plus_emphasis) = compute_subhunk_emphasis(&minus_contents, &plus_contents);

    let mut mi = 0usize;
    let mut pi = 0usize;

    for line in lines {
        match line.kind {
            LineKind::Removed => {
                rendered.push(render_emphasized_line(
                    &line.content,
                    &minus_emphasis[mi],
                    LineKind::Removed,
                    highlight,
                    width,
                    theme,
                ));
                mi += 1;
            }
            LineKind::Added => {
                rendered.push(render_emphasized_line(
                    &line.content,
                    &plus_emphasis[pi],
                    LineKind::Added,
                    highlight,
                    width,
                    theme,
                ));
                pi += 1;
            }
            // `Hunk::runs::Change` only emits Added/Removed; Context
            // lines arrive separately as `HunkRun::Context`.
            LineKind::Context => {}
        }
    }
}

/// Render a single diff line with emphasis information.
///
/// `kind` selects which theme background pair to use; only `Added` and
/// `Removed` reach this function (subhunks emit those exclusively).
fn render_emphasized_line(
    content: &str,
    emphasis: &LineEmphasis,
    kind: LineKind,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (plain_bg, emph_bg) = match kind {
        LineKind::Added => (
            rgb_to_color(theme.diff_added_bg),
            rgb_to_color(theme.diff_added_emph_bg),
        ),
        LineKind::Removed => (
            rgb_to_color(theme.diff_deleted_bg),
            rgb_to_color(theme.diff_deleted_emph_bg),
        ),
        // Subhunks never emit context lines; render flat for safety.
        LineKind::Context => (Color::Reset, Color::Reset),
    };

    match emphasis {
        LineEmphasis::Plain => syntax_diff_line(content, plain_bg, highlight, width, theme),
        LineEmphasis::Paired(sections) => {
            let bg_for_section = |section: &EmphSection| -> Color {
                match section.kind {
                    EmphKind::Emph => emph_bg,
                    EmphKind::NonEmph => plain_bg,
                }
            };
            let (mut spans, visual_width) = highlighted_spans_with_emphasis(
                theme,
                highlight,
                content,
                sections,
                bg_for_section,
                width,
            );
            let padding = width.saturating_sub(visual_width);
            if padding > 0 {
                spans.push(Span::styled(
                    " ".repeat(padding),
                    Style::default().bg(plain_bg),
                ));
            }
            Line::from(spans)
        }
    }
}

/// Render a context (or unemphasised) line with optional background.
fn syntax_diff_line(
    content: &str,
    bg: Color,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let base_style = Style::default().bg(bg);

    let (mut spans, visual_width) = highlighted_spans(theme, highlight, content, base_style, width);
    let padding = width.saturating_sub(visual_width);
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), base_style));
    }

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Syntax highlighting → ratatui spans
// ---------------------------------------------------------------------------

/// Convert syntect foreground color to ratatui Color.
///
/// The "ansi" theme encodes ANSI color indices specially:
/// - `r=N, g=0, b=0, a=0` means ANSI color N (0-15)
/// - `r=0, g=0, b=0, a=1` means default foreground
fn syntect_to_ratatui_color(color: syntect::highlighting::Color) -> Color {
    if color.g == 0 && color.b == 0 && color.a == 0 && color.r <= 15 {
        return Color::Indexed(color.r);
    }
    if color.r == 0 && color.g == 0 && color.b == 0 && color.a == 1 {
        return Color::Reset;
    }
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

fn highlighted_spans(
    theme_ignored: &Theme,
    highlight: Option<&str>,
    line: &str,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let _ = theme_ignored; // accepted for symmetry with the emphasis variant
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => truncate_highlighted_ranges(ranges, base_style, max_width),
        Err(_) => plain_text_spans(line, base_style, max_width),
    }
}

/// Produce syntax-highlighted spans with per-section background colors.
///
/// Each `EmphSection` maps to a substring of `line`. The `bg_for_section`
/// callback returns the background color for that section. Syntax foreground
/// colors are preserved.
fn highlighted_spans_with_emphasis(
    _theme: &Theme,
    highlight: Option<&str>,
    line: &str,
    sections: &[EmphSection],
    bg_for_section: impl Fn(&EmphSection) -> Color,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    let section_ranges = build_section_byte_ranges(sections);

    match highlighter.highlight_line(line, assets.syntax_set) {
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
    bg_for_section: &impl Fn(&EmphSection) -> Color,
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
                .unwrap_or(Color::Reset);
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
    bg_for_section: &impl Fn(&EmphSection) -> Color,
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
            .unwrap_or(Color::Reset);
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

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if ch == '\t' {
                TAB_WIDTH
            } else {
                ch.width().unwrap_or(0)
            }
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Pane helpers (shared between edit-tui and rv)
// ---------------------------------------------------------------------------
//
// These build the rounded, titled, optionally-footered [`Block`] used to
// frame each pane in our TUIs, plus the matching scrollbar widget. Living
// here keeps the two binaries visually identical without making either
// of them depend on the other.

/// Pick the border colour for a pane based on whether it currently has
/// focus. Active panes use [`Theme::border_active`] (the bright accent),
/// inactive panes use [`Theme::border`].
pub fn pane_border_color(active: bool, theme: &Theme) -> Color {
    if active {
        rgb_to_color(theme.border_active)
    } else {
        rgb_to_color(theme.border)
    }
}

/// Inner height of a pane block (its area minus the two border rows).
pub fn pane_inner_height(area: Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

/// Inner width of a pane block (its area minus the two border columns).
pub fn pane_inner_width(area: Rect) -> usize {
    area.width.saturating_sub(2) as usize
}

/// Build a rounded-border [`Block`] with the given title and border
/// colour. Use [`pane_block_with_footer`] when you also want a
/// bottom-right counter.
pub fn pane_block(title: &'static str, color: Color) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
}

/// Like [`pane_block`] but also renders a right-aligned footer string
/// inside the bottom border. Pass `None` to skip the footer.
pub fn pane_block_with_footer(
    title: &'static str,
    color: Color,
    footer: Option<String>,
) -> Block<'static> {
    let mut block = pane_block(title, color);
    if let Some(footer) = footer {
        block = block.title_bottom(Line::from(footer).right_aligned());
    }
    block
}

/// Format a position counter for the pane footer: `" 3 of 12 "`.
pub fn position_footer(position: usize, total: usize) -> String {
    format!(" {position} of {total} ")
}

/// Render a vertical scrollbar inside the right border of `area`,
/// styled with the theme's border colour. No-op when the content fits
/// the viewport.
///
/// `content_length` is the number of logical rows of content;
/// `position` is the current top-row index (or selected-row index in a
/// list); `viewport` is the inner pane height. The scrollbar uses the
/// inner area (1-row vertical margin) so the thumb stays clear of the
/// rounded corners.
pub fn render_pane_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_length: usize,
    position: usize,
    viewport: usize,
    theme: &Theme,
) {
    if content_length <= viewport.max(1) {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(scrollbar_symbols::VERTICAL)
        .thumb_symbol("\u{2590}")
        .track_style(Style::default().fg(rgb_to_color(theme.border)))
        .thumb_style(Style::default().fg(rgb_to_color(theme.border)))
        .begin_symbol(None)
        .end_symbol(None);
    // Ratatui only puts the thumb at the track bottom when position ==
    // content_length - 1. Our scroll offsets max out at
    // content_length - viewport, so pass max_scroll + 1 as the content
    // length and clamp the position accordingly. This makes the thumb
    // reach the bottom for both offset-based and selection-based panes.
    let max_scroll = content_length.saturating_sub(viewport);
    let mut scrollbar_state = ScrollbarState::new(max_scroll.saturating_add(1))
        .position(position.min(max_scroll))
        .viewport_content_length(viewport);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scrollbar_state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diff, DiffLine, Language};

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

    /// Concatenate the visible text of a `Line<'static>` (ignoring styles).
    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn file_header_has_two_lines_with_path_and_separator() {
        let theme = Theme::default();
        let lines = render_file_header("src/main.rs", 80, &theme);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "src/main.rs");
        assert!(line_text(&lines[1]).contains("───"));
    }

    #[test]
    fn rename_header_shows_arrow() {
        let theme = Theme::default();
        let line = render_rename_header("old.rs", "new.rs", &theme);
        let text = line_text(&line);
        assert!(text.contains("renamed:"));
        assert!(text.contains("old.rs"));
        assert!(text.contains("new.rs"));
        assert!(text.contains("⟶"));
    }

    #[test]
    fn render_hunk_emits_breadcrumb_when_ancestors_present() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        // First line is the breadcrumb top border.
        assert!(line_text(&lines[0]).contains("╮"));
        // Some line in the box references the ancestor.
        assert!(lines.iter().any(|l| line_text(l).contains("fn main()")));
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        // Expect a 3-line box containing the line number.
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert!(text.iter().any(|t| t.contains("42")));
        assert!(text[0].contains("╮"));
    }

    #[test]
    fn render_hunk_added_line_carries_added_bg() {
        let theme = Theme::default();
        let hunk = rust_function_hunk();
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        let added_bg = rgb_to_color(theme.diff_added_bg);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.bg == Some(added_bg))),
            "expected at least one span with the added background"
        );
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        let added_emph = rgb_to_color(theme.diff_added_emph_bg);
        let removed_emph = rgb_to_color(theme.diff_deleted_emph_bg);
        let bgs: Vec<Color> = lines
            .iter()
            .flat_map(|l| l.spans.iter().filter_map(|s| s.style.bg))
            .collect();
        assert!(bgs.contains(&added_emph), "missing added emph bg");
        assert!(bgs.contains(&removed_emph), "missing removed emph bg");
    }

    #[test]
    fn render_hunk_uses_language_for_extensionless_path() {
        // Mirror the ANSI renderer's regression test: path-detected languages
        // must drive highlighting even for files without an extension.
        let original = "#!/usr/bin/env bash\n\nfunction name() {\n  echo old\n}\n";
        let updated = "#!/usr/bin/env bash\n\nfunction name() {\n  echo new\n}\n";

        let with_ext = Diff::compute(original, updated, "script.sh");
        let no_ext = Diff::compute(original, updated, "script");
        assert_eq!(with_ext.language(), Some(Language::Bash));
        assert_eq!(no_ext.language(), Some(Language::Bash));
        assert_eq!(with_ext.highlight(), no_ext.highlight());

        let theme = Theme::default();
        let render = |diff: &Diff| -> Vec<Line<'static>> {
            diff.hunks()
                .iter()
                .flat_map(|h| render_hunk(h, diff.highlight(), 120, &theme))
                .collect()
        };

        let a: Vec<String> = render(&with_ext).iter().map(line_text).collect();
        let b: Vec<String> = render(&no_ext).iter().map(line_text).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn render_hunk_dissimilar_change_skips_emphasis_bg() {
        // Mirror the ANSI renderer's `render_hunk_dissimilar_change_skips_emphasis_bg`:
        // when removed/added lines are too dissimilar to pair, both stay
        // on the plain background and never get the emph background.
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
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        let added_emph = rgb_to_color(theme.diff_added_emph_bg);
        let removed_emph = rgb_to_color(theme.diff_deleted_emph_bg);
        let bgs: Vec<Color> = lines
            .iter()
            .flat_map(|l| l.spans.iter().filter_map(|s| s.style.bg))
            .collect();
        assert!(
            !bgs.contains(&added_emph),
            "unpaired added line must not use the emph bg"
        );
        assert!(
            !bgs.contains(&removed_emph),
            "unpaired removed line must not use the emph bg"
        );
    }

    #[test]
    fn breadcrumb_with_non_adjacent_ancestors_emits_gap_row() {
        // When ancestor scopes are not consecutive lines, the breadcrumb
        // box inserts a `...` row to indicate the gap.
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 80,
            new_start: 80,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "        x = 1;".to_string(),
            }],
            ancestors: vec![
                ScopeNode {
                    kind: "impl_item".to_string(),
                    name: "Foo".to_string(),
                    start_line: 3,
                    end_line: 200,
                    text: "impl Foo {".to_string(),
                },
                ScopeNode {
                    kind: "function_item".to_string(),
                    name: "compute".to_string(),
                    start_line: 75,
                    end_line: 90,
                    text: "    fn compute(&self) -> i32 {".to_string(),
                },
            ],
        };
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|t| t.contains("...")),
            "non-adjacent ancestors should produce a `...` gap row, got: {texts:#?}"
        );
        // Both ancestors still rendered.
        assert!(texts.iter().any(|t| t.contains("impl Foo")));
        assert!(texts.iter().any(|t| t.contains("fn compute")));
    }

    #[test]
    fn breadcrumb_with_adjacent_ancestors_has_no_gap_row() {
        let theme = Theme::default();
        let hunk = Hunk {
            old_start: 5,
            new_start: 5,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "x".to_string(),
            }],
            ancestors: vec![
                ScopeNode {
                    kind: "impl_item".to_string(),
                    name: "Foo".to_string(),
                    start_line: 3,
                    end_line: 50,
                    text: "impl Foo {".to_string(),
                },
                ScopeNode {
                    kind: "function_item".to_string(),
                    name: "new".to_string(),
                    start_line: 4,
                    end_line: 10,
                    text: "    fn new() -> Self {".to_string(),
                },
            ],
        };
        let lines = render_hunk(&hunk, Some("Rust"), 80, &theme);
        for line in &lines {
            assert!(
                !line_text(line).contains("..."),
                "adjacent ancestors should not produce a `...` row"
            );
        }
    }
}
