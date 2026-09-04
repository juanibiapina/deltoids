//! Render diff hunks as semantic HTML for the `deltoids serve` web app.
//!
//! Sibling of [`crate::render`] (ANSI) and [`crate::render_tui`] (ratatui):
//! same hunk model and syntax/intraline pipeline, different output. Emits a
//! small tree of `<div>`/`<span>` elements with stable CSS classes so the
//! web app's stylesheet owns colours for diff backgrounds and emphasis,
//! while syntax-token foregrounds are inlined from syntect.
//!
//! Available only when the `html` cargo feature is enabled.
//!
//! Class contract (styled by the web app's CSS):
//! - `.hunk`            one hunk block
//! - `.gap`             divider between two hunks standing in for the unshown
//!   lines; carries `data-gap-lines` (count) and `data-gap-new-start` /
//!   `data-gap-new-end` / `data-gap-old-start` so a client can render that
//!   range as context rows on demand
//! - `.gap-label`       the "N unmodified lines" text inside a `.gap`
//! - `.breadcrumb`      scope-context header (ancestor opening lines)
//! - `.lineno`          the line-number-only header when a hunk has no scope
//! - `.crumb-lineno`    the hunk start line number shown inside a breadcrumb
//! - `.row`             a body line; carries `.context` / `.added` / `.removed`
//! - `.ln`              the gutter line number inside a row
//! - `.code`            the code cell inside a row; the stylesheet owns
//!   wrapping (soft wrap with a hanging indent by default), so rows must
//!   stay a single `.ln` + `.code` pair with no pre-wrapped line breaks
//! - `.emph`            an intraline-emphasised span inside `.code`
//! - `[data-first-change]` marks the first changed row of the entry so the
//!   web app can scroll it to the vertical centre.

use syntect::highlighting::Style as SyntectStyle;
use syntect::highlighting::Theme as SyntectTheme;

use crate::highlight::HunkHighlighter;
use crate::intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
use crate::{DiffLine, Hunk, HunkRun, LineKind, ScopeNode};

/// Render a list of hunks as the HTML diff body for one trace entry.
///
/// `highlight` is the syntect syntax name (from `Diff::highlight()` /
/// the stored trace entry). `syntax_theme` is a registry theme name resolved
/// through [`crate::theme_by_name`]; `None` uses the default. The returned
/// string is the inner HTML the web app injects into its diff container. The
/// first changed row across all hunks carries a `data-first-change` attribute.
pub fn render_entry_html(
    hunks: &[Hunk],
    highlight: Option<&str>,
    syntax_theme: Option<&str>,
) -> String {
    render_entry_html_inner(hunks, highlight, syntax_theme, None)
}

/// Like [`render_entry_html`], but also emits a trailing gap divider down to
/// `total_new_lines` (the new-file line count) when the last hunk stops before
/// end of file. Callers that know the file length — the web reviewer, which
/// holds the after content — use this so the end-of-file unshown lines are
/// shown and expandable too. Callers rendering from stored hunks alone (e.g.
/// `deltoids serve`) use [`render_entry_html`], which omits the trailing gap.
pub fn render_entry_html_with_file_len(
    hunks: &[Hunk],
    highlight: Option<&str>,
    syntax_theme: Option<&str>,
    total_new_lines: usize,
) -> String {
    render_entry_html_inner(hunks, highlight, syntax_theme, Some(total_new_lines))
}

fn render_entry_html_inner(
    hunks: &[Hunk],
    highlight: Option<&str>,
    syntax_theme: Option<&str>,
    total_new_lines: Option<usize>,
) -> String {
    let theme = crate::theme_by_name(syntax_theme);
    let mut html = String::new();
    let mut first_change_emitted = false;

    // Leading gap: unshown lines above the first hunk. Before the first change
    // both sides are in lockstep, so the old side starts at the same offset.
    if let Some(first) = hunks.first()
        && first.new_start > 1
    {
        let old_start = 1 + first.old_start.saturating_sub(first.new_start);
        render_gap(1, old_start, first.new_start - 1, &mut html);
    }

    // End (exclusive, 1-based) of the previous hunk in new/old line space, so
    // the gap before the next hunk is a pure function of the hunk list.
    let mut prev_end: Option<(usize, usize)> = None;
    for hunk in hunks {
        if let Some((prev_new_end, prev_old_end)) = prev_end {
            let count = hunk.new_start.saturating_sub(prev_new_end);
            if count > 0 {
                render_gap(prev_new_end, prev_old_end, count, &mut html);
            }
        }
        render_hunk_html(hunk, highlight, theme, &mut first_change_emitted, &mut html);
        prev_end = Some((
            hunk.new_start + hunk_new_span(hunk),
            hunk.old_start + hunk_old_span(hunk),
        ));
    }

    // Trailing gap: unshown lines below the last hunk, when the file length is
    // known.
    if let (Some((prev_new_end, prev_old_end)), Some(total)) = (prev_end, total_new_lines)
        && total >= prev_new_end
    {
        render_gap(
            prev_new_end,
            prev_old_end,
            total - prev_new_end + 1,
            &mut html,
        );
    }

    html
}

/// Number of new-file lines a hunk advances through (added + context).
fn hunk_new_span(hunk: &Hunk) -> usize {
    hunk.lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Added | LineKind::Context))
        .count()
}

/// Number of old-file lines a hunk advances through (removed + context).
fn hunk_old_span(hunk: &Hunk) -> usize {
    hunk.lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Removed | LineKind::Context))
        .count()
}

/// Render the divider that stands in for the unshown lines between two hunks.
///
/// `new_start` / `old_start` are the first unshown line (1-based) on each side;
/// `count` is how many new-file lines are skipped. The `data-gap-*` attributes
/// carry the exact new-file range so a client can render that range as context
/// rows on demand. `data-gap-old-start` is recorded for symmetry; expanding
/// only needs the new-side range.
fn render_gap(new_start: usize, old_start: usize, count: usize, html: &mut String) {
    let new_end = new_start + count - 1;
    html.push_str("<div class=\"gap\" data-gap-lines=\"");
    html.push_str(&count.to_string());
    html.push_str("\" data-gap-new-start=\"");
    html.push_str(&new_start.to_string());
    html.push_str("\" data-gap-new-end=\"");
    html.push_str(&new_end.to_string());
    html.push_str("\" data-gap-old-start=\"");
    html.push_str(&old_start.to_string());
    html.push_str("\"><span class=\"gap-label\">");
    html.push_str(&count.to_string());
    html.push_str(if count == 1 {
        " unmodified line"
    } else {
        " unmodified lines"
    });
    html.push_str("</span></div>");
}

/// Render new-file lines `start..=end` (1-based, inclusive) of `after` as
/// context rows, so a client can reveal the unshown lines a `.gap` divider
/// stands in for. Rows reuse the same `.row.context` markup, gutter, and
/// syntect highlighting as the diff body.
///
/// The range is clamped to the content; an empty or reversed range renders
/// nothing. `highlight` is the syntect syntax name and `syntax_theme` a
/// registry theme name (`None` = default), matching [`render_entry_html`].
///
/// Highlighting starts fresh at `start`, so a range that opens mid block
/// comment or string may mis-colour — acceptable for revealed context.
pub fn render_context_html(
    after: &str,
    highlight: Option<&str>,
    syntax_theme: Option<&str>,
    start: usize,
    end: usize,
) -> String {
    if start == 0 || end < start {
        return String::new();
    }
    let lines: Vec<&str> = after.lines().collect();
    if start > lines.len() {
        return String::new();
    }
    let end = end.min(lines.len());

    let theme = crate::theme_by_name(syntax_theme);
    let mut highlighter = HunkHighlighter::new(highlight, theme);
    let mut html = String::new();
    // Context rows never carry the first-change marker; keep it "already
    // emitted" so render_row can never add one.
    let mut suppress_marker = true;
    for number in start..=end {
        let line = DiffLine {
            kind: LineKind::Context,
            content: lines[number - 1].to_string(),
        };
        let ranges = highlighter.context(&line.content);
        render_row(
            &line,
            Some(number),
            &ranges,
            None,
            &mut suppress_marker,
            &mut html,
        );
    }
    html
}

fn render_hunk_html(
    hunk: &Hunk,
    highlight: Option<&str>,
    syntax_theme: &'static SyntectTheme,
    first_change_emitted: &mut bool,
    html: &mut String,
) {
    html.push_str("<div class=\"hunk\">");
    render_header(hunk, html);

    let mut highlighter = HunkHighlighter::new(highlight, syntax_theme);
    let mut new_line = hunk.new_start;
    let mut old_line = hunk.old_start;
    for run in hunk.runs() {
        match run {
            HunkRun::Context(line) => {
                let ranges = highlighter.context(&line.content);
                render_row(
                    line,
                    Some(new_line),
                    &ranges,
                    None,
                    first_change_emitted,
                    html,
                );
                new_line += 1;
                old_line += 1;
            }
            HunkRun::Change(slice) => {
                render_change(
                    slice,
                    &mut highlighter,
                    &mut old_line,
                    &mut new_line,
                    first_change_emitted,
                    html,
                );
            }
        }
    }

    html.push_str("</div>");
}

/// Render the hunk header: the ancestor scope breadcrumb, or a plain line
/// number when the hunk has no enclosing structural scope.
fn render_header(hunk: &Hunk, html: &mut String) {
    if hunk.ancestors.is_empty() {
        html.push_str("<div class=\"lineno\">");
        html.push_str(&hunk.new_start.to_string());
        html.push_str("</div>");
        return;
    }

    html.push_str("<div class=\"breadcrumb\">");
    html.push_str("<span class=\"crumb-lineno\">");
    html.push_str(&hunk.new_start.to_string());
    html.push_str("</span>");
    for (index, ancestor) in hunk.ancestors.iter().enumerate() {
        if index > 0 {
            html.push_str("<span class=\"crumb-sep\"> \u{203a} </span>");
        }
        render_crumb(ancestor, html);
    }
    html.push_str("</div>");
}

fn render_crumb(ancestor: &ScopeNode, html: &mut String) {
    // Prefer the scope's name; fall back to the trimmed opening line.
    let label = if ancestor.name.is_empty() {
        ancestor.text.trim()
    } else {
        ancestor.name.as_str()
    };
    html.push_str("<span class=\"crumb\">");
    push_escaped(html, label);
    html.push_str("</span>");
}

/// Render a maximal run of consecutive Added/Removed lines, pairing them for
/// intraline emphasis (the same pairing the ANSI/TUI renderers use).
fn render_change(
    slice: &[DiffLine],
    highlighter: &mut HunkHighlighter,
    old_line: &mut usize,
    new_line: &mut usize,
    first_change_emitted: &mut bool,
    html: &mut String,
) {
    let minus: Vec<&str> = slice
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .map(|l| l.content.as_str())
        .collect();
    let plus: Vec<&str> = slice
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.as_str())
        .collect();
    let (minus_emphasis, plus_emphasis) = compute_subhunk_emphasis(&minus, &plus);

    let mut mi = 0usize;
    let mut pi = 0usize;
    for line in slice {
        match line.kind {
            LineKind::Removed => {
                let ranges = highlighter.removed(&line.content);
                render_row(
                    line,
                    Some(*old_line),
                    &ranges,
                    Some(&minus_emphasis[mi]),
                    first_change_emitted,
                    html,
                );
                *old_line += 1;
                mi += 1;
            }
            LineKind::Added => {
                let ranges = highlighter.added(&line.content);
                render_row(
                    line,
                    Some(*new_line),
                    &ranges,
                    Some(&plus_emphasis[pi]),
                    first_change_emitted,
                    html,
                );
                *new_line += 1;
                pi += 1;
            }
            LineKind::Context => {}
        }
    }
}

fn row_class(kind: &LineKind) -> &'static str {
    match kind {
        LineKind::Added => "row added",
        LineKind::Removed => "row removed",
        LineKind::Context => "row context",
    }
}

fn render_row(
    line: &DiffLine,
    line_number: Option<usize>,
    ranges: &[(SyntectStyle, &str)],
    emphasis: Option<&LineEmphasis>,
    first_change_emitted: &mut bool,
    html: &mut String,
) {
    let is_change = !matches!(line.kind, LineKind::Context);
    html.push_str("<div class=\"");
    html.push_str(row_class(&line.kind));
    html.push('"');
    if is_change && !*first_change_emitted {
        html.push_str(" data-first-change");
        *first_change_emitted = true;
    }
    html.push('>');

    html.push_str("<span class=\"ln\">");
    if let Some(number) = line_number {
        html.push_str(&number.to_string());
    }
    html.push_str("</span>");

    html.push_str("<span class=\"code\">");
    render_code(ranges, emphasis, html);
    html.push_str("</span>");

    html.push_str("</div>");
}

/// Render the code cell: merge syntect token colours with intraline emphasis
/// so each output span carries a single (colour, emph) pair.
fn render_code(
    ranges: &[(SyntectStyle, &str)],
    emphasis: Option<&LineEmphasis>,
    html: &mut String,
) {
    let emph_ranges = emphasis.map(emph_byte_ranges).unwrap_or_default();

    let mut byte = 0usize;
    let mut open: Option<(Option<[u8; 3]>, bool)> = None;
    let mut buffer = String::new();

    for (style, text) in ranges {
        let colour = foreground_rgb(style);
        for ch in text.chars() {
            let emph = byte_is_emph(byte, &emph_ranges);
            let key = (colour, emph);
            if open != Some(key) {
                flush_span(&mut buffer, open, html);
                open = Some(key);
            }
            buffer.push(ch);
            byte += ch.len_utf8();
        }
    }
    flush_span(&mut buffer, open, html);
}

fn flush_span(buffer: &mut String, open: Option<(Option<[u8; 3]>, bool)>, html: &mut String) {
    if buffer.is_empty() {
        return;
    }
    let Some((colour, emph)) = open else {
        buffer.clear();
        return;
    };
    let has_span = colour.is_some() || emph;
    if has_span {
        html.push_str("<span");
        if emph {
            html.push_str(" class=\"emph\"");
        }
        if let Some([r, g, b]) = colour {
            html.push_str(&format!(" style=\"color:#{r:02x}{g:02x}{b:02x}\""));
        }
        html.push('>');
    }
    push_escaped(html, buffer);
    if has_span {
        html.push_str("</span>");
    }
    buffer.clear();
}

/// Byte ranges (start, end) of the emphasised sections of a paired line.
fn emph_byte_ranges(emphasis: &LineEmphasis) -> Vec<(usize, usize)> {
    let LineEmphasis::Paired(sections) = emphasis else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut byte = 0usize;
    for EmphSection { kind, text } in sections {
        let len = text.len();
        if matches!(kind, EmphKind::Emph) {
            ranges.push((byte, byte + len));
        }
        byte += len;
    }
    ranges
}

fn byte_is_emph(byte: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| byte >= start && byte < end)
}

/// syntect foreground colour as RGB, unless it is the theme's default
/// foreground (which we leave to CSS so themes stay consistent).
fn foreground_rgb(style: &SyntectStyle) -> Option<[u8; 3]> {
    let colour = style.foreground;
    // The "ansi" theme encodes the default foreground as r=g=b=0, a=1.
    if colour.r == 0 && colour.g == 0 && colour.b == 0 && colour.a == 1 {
        return None;
    }
    Some([colour.r, colour.g, colour.b])
}

fn push_escaped(html: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            '\'' => html.push_str("&#39;"),
            _ => html.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffLine, Hunk, LineKind, ScopeNode};

    fn line(kind: LineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
        }
    }

    #[test]
    fn context_only_hunk_has_no_first_change_marker() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Context, "let x = 1;")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None, None);
        assert!(html.contains("class=\"row context\""));
        assert!(!html.contains("data-first-change"));
        // No scope: line-number header.
        assert!(html.contains("class=\"lineno\""));
    }

    #[test]
    fn first_change_marker_lands_on_first_changed_row_only() {
        let hunk = Hunk {
            old_start: 5,
            new_start: 5,
            lines: vec![
                line(LineKind::Context, "context"),
                line(LineKind::Removed, "old"),
                line(LineKind::Added, "new"),
                line(LineKind::Added, "new2"),
            ],
            ancestors: vec![ScopeNode {
                kind: "function_item".to_string(),
                name: "my_func".to_string(),
                start_line: 3,
                end_line: 10,
                text: "fn my_func() {".to_string(),
            }],
        };
        let html = render_entry_html(&[hunk], None, None);
        assert_eq!(html.matches("data-first-change").count(), 1);
        // The marker is on a removed row, before the added rows.
        let marker = html.find("data-first-change").unwrap();
        let removed = html.find("class=\"row removed\"").unwrap();
        assert!(removed <= marker && marker < html.find("class=\"row added\"").unwrap());
        // Breadcrumb shows the scope name and the hunk start line number.
        assert!(html.contains("class=\"breadcrumb\""));
        assert!(html.contains("my_func"));
        assert!(html.contains("class=\"crumb-lineno\">5</span>"));
    }

    #[test]
    fn html_special_characters_are_escaped() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "if a < b && c > d {")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None, None);
        assert!(html.contains("&lt;"));
        assert!(html.contains("&gt;"));
        assert!(html.contains("&amp;"));
        assert!(!html.contains("< b"));
    }

    // -----------------------------------------------------------------------
    // render_context_html tests
    // -----------------------------------------------------------------------

    #[test]
    fn context_html_renders_requested_range_as_context_rows() {
        let after = "one\ntwo\nthree\nfour\nfive\n";
        let html = render_context_html(after, None, None, 2, 4);
        // Three context rows for lines 2, 3, 4.
        assert_eq!(html.matches("class=\"row context\"").count(), 3);
        assert!(html.contains(">2</span>"));
        assert!(html.contains(">3</span>"));
        assert!(html.contains(">4</span>"));
        assert!(html.contains("two"));
        assert!(html.contains("three"));
        assert!(html.contains("four"));
        // Lines outside the range are not rendered.
        assert!(!html.contains(">one<") && !html.contains(">1</span>"));
        assert!(!html.contains("five"));
        // Context expansion never marks a first change.
        assert!(!html.contains("data-first-change"));
    }

    #[test]
    fn context_html_clamps_range_past_end_of_file() {
        let after = "a\nb\nc\n";
        let html = render_context_html(after, None, None, 2, 99);
        // Only lines 2 and 3 exist.
        assert_eq!(html.matches("class=\"row context\"").count(), 2);
        assert!(html.contains(">b<") || html.contains(">b</span>"));
        assert!(html.contains(">c<") || html.contains(">c</span>"));
    }

    #[test]
    fn context_html_empty_range_is_empty() {
        let after = "a\nb\nc\n";
        assert_eq!(render_context_html(after, None, None, 3, 2), "");
        assert_eq!(render_context_html(after, None, None, 0, 0), "");
    }

    // -----------------------------------------------------------------------
    // Gap divider tests
    // -----------------------------------------------------------------------

    /// Two hunks separated by unshown lines emit exactly one `.gap` divider
    /// carrying the skipped-line count and the new/old range it covers.
    #[test]
    fn gap_divider_between_hunks_reports_skipped_count_and_range() {
        // First hunk covers new lines 1..=2 (context + added), second hunk
        // starts at new line 10, so 7 lines (3..=9) are unshown.
        let first = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                line(LineKind::Context, "alpha"),
                line(LineKind::Added, "beta"),
            ],
            ancestors: Vec::new(),
        };
        let second = Hunk {
            old_start: 9,
            new_start: 10,
            lines: vec![line(LineKind::Context, "omega")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[first, second], None, None);

        assert_eq!(html.matches("class=\"gap\"").count(), 1);
        assert!(html.contains("data-gap-lines=\"7\""));
        // Gap starts at the first unshown new line (3) and ends before the
        // next hunk (9).
        assert!(html.contains("data-gap-new-start=\"3\""));
        assert!(html.contains("data-gap-new-end=\"9\""));
        assert!(html.contains("7 unmodified lines"));
        // The divider sits before the second hunk's header.
        let gap = html.find("class=\"gap\"").unwrap();
        let second_hunk = html.rfind("class=\"hunk\"").unwrap();
        assert!(gap < second_hunk);
    }

    #[test]
    fn leading_gap_divider_covers_lines_above_first_hunk() {
        // First hunk starts at new line 5, so lines 1..=4 are unshown above it.
        let hunk = Hunk {
            old_start: 5,
            new_start: 5,
            lines: vec![line(LineKind::Added, "x")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None, None);
        assert_eq!(html.matches("class=\"gap\"").count(), 1);
        assert!(html.contains("data-gap-lines=\"4\""));
        assert!(html.contains("data-gap-new-start=\"1\""));
        assert!(html.contains("data-gap-new-end=\"4\""));
        // The leading divider sits before the hunk.
        assert!(html.find("class=\"gap\"").unwrap() < html.find("class=\"hunk\"").unwrap());
    }

    #[test]
    fn first_hunk_at_line_one_has_no_leading_gap() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "x")],
            ancestors: Vec::new(),
        };
        assert!(!render_entry_html(&[hunk], None, None).contains("class=\"gap\""));
    }

    #[test]
    fn trailing_gap_divider_covers_lines_below_last_hunk() {
        // Hunk covers new lines 1..=3; the file has 10 lines, so 4..=10 (7
        // lines) are unshown below it. Only the file-length-aware renderer
        // emits this.
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                line(LineKind::Context, "a"),
                line(LineKind::Context, "b"),
                line(LineKind::Added, "c"),
            ],
            ancestors: Vec::new(),
        };
        let html = render_entry_html_with_file_len(&[hunk], None, None, 10);
        assert_eq!(html.matches("class=\"gap\"").count(), 1);
        assert!(html.contains("data-gap-lines=\"7\""));
        assert!(html.contains("data-gap-new-start=\"4\""));
        assert!(html.contains("data-gap-new-end=\"10\""));
        // The trailing divider sits after the hunk.
        assert!(html.rfind("class=\"gap\"").unwrap() > html.find("class=\"hunk\"").unwrap());
    }

    #[test]
    fn last_hunk_reaching_eof_has_no_trailing_gap() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Context, "a"), line(LineKind::Added, "b")],
            ancestors: Vec::new(),
        };
        // Hunk covers new lines 1..=2 and the file is 2 lines: nothing below.
        assert!(!render_entry_html_with_file_len(&[hunk], None, None, 2).contains("class=\"gap\""));
    }

    #[test]
    fn plain_render_omits_trailing_gap() {
        // render_entry_html has no file length, so it never emits a trailing
        // gap even when the last hunk stops early.
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "a")],
            ancestors: Vec::new(),
        };
        assert!(!render_entry_html(&[hunk], None, None).contains("class=\"gap\""));
    }

    #[test]
    fn single_hunk_has_no_gap_divider() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "solo")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None, None);
        assert!(!html.contains("class=\"gap\""));
    }

    #[test]
    fn abutting_hunks_have_no_gap_divider() {
        // First hunk covers new lines 1..=2; second starts at new line 3, so
        // there is nothing unshown between them.
        let first = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Context, "a"), line(LineKind::Added, "b")],
            ancestors: Vec::new(),
        };
        let second = Hunk {
            old_start: 2,
            new_start: 3,
            lines: vec![line(LineKind::Added, "c")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[first, second], None, None);
        assert!(!html.contains("class=\"gap\""));
    }

    #[test]
    fn gap_of_one_line_uses_singular_label() {
        let first = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "a")],
            ancestors: Vec::new(),
        };
        let second = Hunk {
            old_start: 3,
            new_start: 3,
            lines: vec![line(LineKind::Added, "c")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[first, second], None, None);
        assert!(html.contains("1 unmodified line<"));
    }

    #[test]
    fn intraline_emphasis_wraps_changed_span() {
        // A paired single-word change should mark the differing token with
        // the emph class on both sides.
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                line(LineKind::Removed, "const x = 1;"),
                line(LineKind::Added, "const x = 2;"),
            ],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None, None);
        assert!(html.contains("class=\"emph\""));
    }
}
