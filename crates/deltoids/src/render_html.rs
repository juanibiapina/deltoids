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
//! - `.breadcrumb`      scope-context header (ancestor opening lines)
//! - `.lineno`          the line-number-only header when a hunk has no scope
//! - `.row`             a body line; carries `.context` / `.added` / `.removed`
//! - `.ln`              the gutter line number inside a row
//! - `.code`            the code cell inside a row
//! - `.emph`            an intraline-emphasised span inside `.code`
//! - `[data-first-change]` marks the first changed row of the entry so the
//!   web app can scroll it to the vertical centre.

use syntect::highlighting::Style as SyntectStyle;

use crate::highlight::HunkHighlighter;
use crate::intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
use crate::{DiffLine, Hunk, HunkRun, LineKind, ScopeNode};

/// Render a list of hunks as the HTML diff body for one trace entry.
///
/// `highlight` is the syntect syntax name (from `Diff::highlight()` /
/// the stored trace entry). The returned string is the inner HTML the web
/// app injects into its diff container. The first changed row across all
/// hunks carries a `data-first-change` attribute.
pub fn render_entry_html(hunks: &[Hunk], highlight: Option<&str>) -> String {
    let mut html = String::new();
    let mut first_change_emitted = false;
    for hunk in hunks {
        render_hunk_html(hunk, highlight, &mut first_change_emitted, &mut html);
    }
    html
}

fn render_hunk_html(
    hunk: &Hunk,
    highlight: Option<&str>,
    first_change_emitted: &mut bool,
    html: &mut String,
) {
    html.push_str("<div class=\"hunk\">");
    render_header(hunk, html);

    let mut highlighter = HunkHighlighter::new(highlight);
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
        let html = render_entry_html(&[hunk], None);
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
        let html = render_entry_html(&[hunk], None);
        assert_eq!(html.matches("data-first-change").count(), 1);
        // The marker is on a removed row, before the added rows.
        let marker = html.find("data-first-change").unwrap();
        let removed = html.find("class=\"row removed\"").unwrap();
        assert!(removed <= marker && marker < html.find("class=\"row added\"").unwrap());
        // Breadcrumb shows the scope name.
        assert!(html.contains("class=\"breadcrumb\""));
        assert!(html.contains("my_func"));
    }

    #[test]
    fn html_special_characters_are_escaped() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineKind::Added, "if a < b && c > d {")],
            ancestors: Vec::new(),
        };
        let html = render_entry_html(&[hunk], None);
        assert!(html.contains("&lt;"));
        assert!(html.contains("&gt;"));
        assert!(html.contains("&amp;"));
        assert!(!html.contains("< b"));
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
        let html = render_entry_html(&[hunk], None);
        assert!(html.contains("class=\"emph\""));
    }
}
