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
//!   hunk (context + intraline-emphasised subhunks). Body lines longer than
//!   `width` wrap onto continuation rows (hard char wrap); the breadcrumb box
//!   still truncates ancestor text to its fixed geometry.
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
use crate::hunk_header::{Breadcrumb, BreadcrumbRow, HunkHeader};
use crate::intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
use crate::{Hunk, HunkRun, LineKind};

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
                output.extend(syntax_diff_line(
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

/// Paint the breadcrumb box from a shared [`Breadcrumb`] plan. Geometry comes
/// from the plan; this function only paints ratatui spans (rounded corners,
/// ancestor text truncated to the box width).
fn render_breadcrumb_box(
    b: &Breadcrumb,
    highlight: Option<&str>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let border = Style::default().fg(rgb_to_color(theme.border));
    let num_col_width = b.num_col_width;
    let prefix_width = b.prefix_width();
    let content_width = b.content_width;

    let top = format!("{}╮", "─".repeat(content_width + 1));
    let bot = format!("{}╯", "─".repeat(content_width + 1));

    let mut lines = vec![Line::from(Span::styled(top, border))];

    for row in &b.rows {
        match row {
            BreadcrumbRow::Scope { line_num, text } => {
                let num_str = format!("{line_num:>num_col_width$}: ");
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
            BreadcrumbRow::Gap => {
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
                rendered.extend(render_emphasized_line(
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
                rendered.extend(render_emphasized_line(
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
///
/// Lines longer than `width` wrap onto continuation rows (hard char wrap);
/// every emitted row is padded to `width` with the line's plain background.
fn render_emphasized_line(
    content: &str,
    emphasis: &LineEmphasis,
    kind: LineKind,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
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
            if width == 0 {
                return vec![Line::from(Vec::new())];
            }
            let bg_for_section = |section: &EmphSection| -> Color {
                match section.kind {
                    EmphKind::Emph => emph_bg,
                    EmphKind::NonEmph => plain_bg,
                }
            };
            let section_ranges = build_section_byte_ranges(sections);
            let mut sink = WrapSink::new(width, plain_bg);
            produce_emphasized(
                &mut sink,
                highlight,
                content,
                sections,
                &section_ranges,
                &bg_for_section,
            );
            sink.finish()
        }
    }
}

/// Render a context (or unemphasised) line with optional background.
///
/// Lines longer than `width` wrap onto continuation rows (hard char wrap);
/// every emitted row is padded to `width` with `bg`.
fn syntax_diff_line(
    content: &str,
    bg: Color,
    highlight: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let _ = theme;
    if width == 0 {
        return vec![Line::from(Vec::new())];
    }
    let base_style = Style::default().bg(bg);
    let mut sink = WrapSink::new(width, bg);
    produce_highlighted(&mut sink, highlight, content, base_style);
    sink.finish()
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

/// Consumer of a stream of styled visible characters.
///
/// The char-production functions ([`produce_highlighted`] /
/// [`produce_emphasized`]) own tab expansion, zero-width skipping, wide-char
/// handling and per-section background selection; the sink owns the only
/// behaviour that differs between the breadcrumb and the diff body: what to do
/// when the next char would not fit the current row. [`TruncateSink`] stops
/// (single row, capped to the box); [`WrapSink`] starts a new padded row.
trait CharSink {
    /// Accept one styled visible char. Returns `false` to ask the producer to
    /// stop feeding (truncation); `true` to keep going.
    fn accept(&mut self, style: Style, visible: &VisibleChar) -> bool;
}

/// Single-row sink that stops once the next char would exceed `max_width`.
/// Used by the breadcrumb box, whose ancestor text is intentionally cut to
/// fit the fixed box geometry.
struct TruncateSink {
    spans: Vec<Span<'static>>,
    buffer: String,
    current_style: Option<Style>,
    width: usize,
    max_width: usize,
}

impl TruncateSink {
    fn new(max_width: usize) -> Self {
        Self {
            spans: Vec::new(),
            buffer: String::new(),
            current_style: None,
            width: 0,
            max_width,
        }
    }

    fn finish(mut self) -> (Vec<Span<'static>>, usize) {
        flush_styled_text(&mut self.spans, &mut self.buffer, &mut self.current_style);
        (self.spans, self.width)
    }
}

impl CharSink for TruncateSink {
    fn accept(&mut self, style: Style, visible: &VisibleChar) -> bool {
        if self.width + visible.width > self.max_width {
            return false;
        }
        push_styled_text(
            &mut self.spans,
            &mut self.buffer,
            &mut self.current_style,
            style,
            &visible.text,
        );
        self.width += visible.width;
        true
    }
}

/// Multi-row sink that folds the char stream into wrapped rows. When the next
/// char would exceed `max_width` the current row is flushed (and padded to
/// `max_width` with `fill_bg`) and a fresh row begins. Used by the diff body
/// so long lines are shown in full instead of being cut.
struct WrapSink {
    rows: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    buffer: String,
    current_style: Option<Style>,
    width: usize,
    max_width: usize,
    fill_bg: Color,
}

impl WrapSink {
    fn new(max_width: usize, fill_bg: Color) -> Self {
        Self {
            rows: Vec::new(),
            spans: Vec::new(),
            buffer: String::new(),
            current_style: None,
            width: 0,
            max_width,
            fill_bg,
        }
    }

    /// Flush the current row: emit its spans (padded to `max_width`) and reset
    /// for the next row.
    fn flush_row(&mut self) {
        flush_styled_text(&mut self.spans, &mut self.buffer, &mut self.current_style);
        let padding = self.max_width.saturating_sub(self.width);
        if padding > 0 {
            self.spans.push(Span::styled(
                " ".repeat(padding),
                Style::default().bg(self.fill_bg),
            ));
        }
        self.rows.push(Line::from(std::mem::take(&mut self.spans)));
        self.width = 0;
    }

    /// Emit the final (possibly partial or empty) row and return all rows.
    /// Empty content yields exactly one padded row.
    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_row();
        self.rows
    }
}

impl CharSink for WrapSink {
    fn accept(&mut self, style: Style, visible: &VisibleChar) -> bool {
        if self.width + visible.width > self.max_width {
            self.flush_row();
        }
        push_styled_text(
            &mut self.spans,
            &mut self.buffer,
            &mut self.current_style,
            style,
            &visible.text,
        );
        self.width += visible.width;
        true
    }
}

/// Feed one segment of constant-style chars into `sink`. Returns `false` if
/// the sink asked to stop (truncation).
fn feed_segment<S: CharSink>(sink: &mut S, segment: &str, style: Style) -> bool {
    for ch in segment.chars() {
        let Some(visible) = visible_char(ch) else {
            continue;
        };
        if !sink.accept(style, &visible) {
            return false;
        }
    }
    true
}

/// Feed `line`'s syntax-highlighted chars into `sink`, with a constant
/// background from `base_style`. Falls back to plain (single style) when the
/// highlighter errors.
fn produce_highlighted<S: CharSink>(
    sink: &mut S,
    highlight: Option<&str>,
    line: &str,
    base_style: Style,
) {
    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => {
            for (syn_style, segment) in ranges {
                let style = to_ratatui_style(base_style, syn_style);
                if !feed_segment(sink, segment, style) {
                    break;
                }
            }
        }
        Err(_) => {
            feed_segment(sink, line, base_style);
        }
    }
}

/// Feed one segment into `sink`, choosing each char's background from its
/// emphasis section (and optionally layering a syntect foreground). Tracks the
/// running `byte_offset`. Returns `false` if the sink asked to stop.
fn feed_emphasized_segment<S: CharSink>(
    sink: &mut S,
    segment: &str,
    byte_offset: &mut usize,
    syn_style: Option<syntect::highlighting::Style>,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    bg_for_section: &impl Fn(&EmphSection) -> Color,
) -> bool {
    for ch in segment.chars() {
        let Some(visible) = visible_char(ch) else {
            *byte_offset += ch.len_utf8();
            continue;
        };
        let bg = section_index_at(*byte_offset, section_ranges)
            .map(|i| bg_for_section(&sections[i]))
            .unwrap_or(Color::Reset);
        let style = match syn_style {
            Some(syn) => to_ratatui_style(Style::default().bg(bg), syn),
            None => Style::default().bg(bg),
        };
        if !sink.accept(style, &visible) {
            return false;
        }
        *byte_offset += visible.byte_len;
    }
    true
}

/// Feed `line`'s syntax-highlighted chars into `sink`, choosing each char's
/// background from its emphasis section. Syntax foreground colours are kept.
fn produce_emphasized<S: CharSink>(
    sink: &mut S,
    highlight: Option<&str>,
    line: &str,
    sections: &[EmphSection],
    section_ranges: &[(usize, usize)],
    bg_for_section: &impl Fn(&EmphSection) -> Color,
) {
    let assets = SyntaxAssets::load();
    let syntax = assets.syntax_for_name(highlight);
    let mut highlighter = HighlightLines::new(syntax, assets.syntax_theme);

    let mut byte_offset = 0usize;
    match highlighter.highlight_line(line, assets.syntax_set) {
        Ok(ranges) => {
            for (syn_style, segment) in ranges {
                if !feed_emphasized_segment(
                    sink,
                    segment,
                    &mut byte_offset,
                    Some(syn_style),
                    sections,
                    section_ranges,
                    bg_for_section,
                ) {
                    break;
                }
            }
        }
        Err(_) => {
            feed_emphasized_segment(
                sink,
                line,
                &mut byte_offset,
                None,
                sections,
                section_ranges,
                bg_for_section,
            );
        }
    }
}

/// Single-row, truncating highlighted spans. Used by the breadcrumb box,
/// whose ancestor text is intentionally capped to the box width (no wrap).
fn highlighted_spans(
    theme_ignored: &Theme,
    highlight: Option<&str>,
    line: &str,
    base_style: Style,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    let _ = theme_ignored; // accepted for symmetry with the body renderers
    if max_width == 0 {
        return (Vec::new(), 0);
    }
    let mut sink = TruncateSink::new(max_width);
    produce_highlighted(&mut sink, highlight, line, base_style);
    sink.finish()
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
    use crate::{Diff, DiffLine, Language, ScopeNode};

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

    /// Visible width of a `Line<'static>` (sum of span display widths).
    fn line_width(line: &Line<'static>) -> usize {
        line.spans
            .iter()
            .map(|s| display_width(s.content.as_ref()))
            .sum()
    }

    fn context_hunk(content: &str) -> Hunk {
        Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![DiffLine {
                kind: LineKind::Context,
                content: content.to_string(),
            }],
            ancestors: Vec::new(),
        }
    }

    /// Body rows (everything after the 3-line header box).
    fn body_rows(lines: &[Line<'static>]) -> Vec<Line<'static>> {
        lines[3..].to_vec()
    }

    #[test]
    fn long_context_line_wraps_into_multiple_rows() {
        let theme = Theme::default();
        let content = "abcdefghij klmnopqrst uvwxyz0123 456789"; // 39 cols
        let width = 10;
        let hunk = context_hunk(content);
        let lines = render_hunk(&hunk, None, width, &theme);
        let body = body_rows(&lines);
        assert!(body.len() > 1, "expected multiple wrapped rows");
        for row in &body {
            assert!(
                line_width(row) <= width,
                "row wider than width: {:?}",
                line_text(row)
            );
        }
        // Concatenated visible body (trimmed of trailing pad) reproduces content.
        let joined: String = body.iter().map(line_text).collect();
        assert_eq!(joined.trim_end(), content);
    }

    #[test]
    fn wrapped_added_line_pads_every_row_with_added_bg() {
        let theme = Theme::default();
        let width = 8;
        let added_bg = rgb_to_color(theme.diff_added_bg);
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "0123456789abcdefghij".to_string(), // 20 cols
            }],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, None, width, &theme);
        let body = body_rows(&lines);
        assert!(body.len() > 1, "expected wrap");
        for row in &body {
            assert_eq!(line_width(row), width, "each row padded to width");
            assert!(
                row.spans.iter().any(|s| s.style.bg == Some(added_bg)),
                "every wrapped row carries the added background"
            );
        }
    }

    #[test]
    fn exact_width_content_produces_single_row() {
        let theme = Theme::default();
        let width = 10;
        let hunk = context_hunk("0123456789"); // exactly 10 cols
        let lines = render_hunk(&hunk, None, width, &theme);
        let body = body_rows(&lines);
        assert_eq!(
            body.len(),
            1,
            "exact-fit content must not emit a trailing row"
        );
        assert_eq!(line_width(&body[0]), width);
    }

    #[test]
    fn wide_char_at_boundary_moves_whole_to_next_row() {
        let theme = Theme::default();
        let width = 4;
        // Three columns of ASCII then a width-2 char: it cannot fit at col 3,
        // so it moves whole to the next row.
        let hunk = context_hunk("abc世");
        let lines = render_hunk(&hunk, None, width, &theme);
        let body = body_rows(&lines);
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[0]).trim_end(), "abc");
        assert_eq!(line_text(&body[1]).trim_end(), "世");
        for row in &body {
            assert!(line_width(row) <= width);
        }
    }

    #[test]
    fn paired_change_wraps_and_keeps_both_backgrounds() {
        let theme = Theme::default();
        let width = 8;
        let plain_bg = rgb_to_color(theme.diff_added_bg);
        let emph_bg = rgb_to_color(theme.diff_added_emph_bg);
        // A long pair that differs only in the tail so emphasis sits late in
        // the line, forcing an emph section past the first wrap boundary.
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Removed,
                    content: "value_aaaaaaaaaaaa = 1".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "value_aaaaaaaaaaaa = 2".to_string(),
                },
            ],
            ancestors: Vec::new(),
        };
        let lines = render_hunk(&hunk, None, width, &theme);
        // Added rows: every row padded to width, and both bg colors appear
        // across the wrapped rows.
        let added_rows: Vec<&Line<'static>> = lines
            .iter()
            .filter(|l| {
                l.spans
                    .iter()
                    .any(|s| matches!(s.style.bg, Some(b) if b == plain_bg || b == emph_bg))
            })
            .collect();
        assert!(added_rows.len() > 1, "expected the added line to wrap");
        for row in &added_rows {
            assert_eq!(line_width(row), width);
        }
        let bgs: Vec<Color> = added_rows
            .iter()
            .flat_map(|l| l.spans.iter().filter_map(|s| s.style.bg))
            .collect();
        assert!(bgs.contains(&plain_bg), "plain bg present across wrap");
        assert!(bgs.contains(&emph_bg), "emph bg present across wrap");
    }

    #[test]
    fn breadcrumb_ancestor_text_stays_capped_and_unwrapped() {
        let theme = Theme::default();
        let width = 30;
        let long = "fn very_long_function_name_that_exceeds_the_box(arg: SomeLongType) -> Result<(), Error>";
        let hunk = Hunk {
            old_start: 10,
            new_start: 10,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "    x = 1;".to_string(),
            }],
            ancestors: vec![ScopeNode {
                kind: "function_item".to_string(),
                name: "very_long_function_name_that_exceeds_the_box".to_string(),
                start_line: 10,
                end_line: 20,
                text: long.to_string(),
            }],
        };
        let lines = render_hunk(&hunk, Some("Rust"), width, &theme);
        // Exactly one scope row inside the box references the ancestor, and it
        // fits within the box width (truncated, not wrapped).
        let scope_rows: Vec<&Line<'static>> = lines
            .iter()
            .filter(|l| line_text(l).contains("fn very"))
            .collect();
        assert_eq!(scope_rows.len(), 1, "ancestor text must not wrap");
        assert!(line_width(scope_rows[0]) <= width);
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
