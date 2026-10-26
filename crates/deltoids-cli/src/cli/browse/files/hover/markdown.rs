//! A hover's Markdown as popup lines: prose wrapped to the popup's
//! width, code blocks highlighted in the user's syntax theme.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::{render_code_block, rgb_to_color};

use crate::cli::browse::text::display_width;

/// The code a hover is about: how to highlight a code block whose fence
/// names no language, or names this one.
pub(super) struct Code<'a> {
    /// The language's name, such as `Rust`.
    pub(super) language: &'a str,
    /// The syntect syntax its files are highlighted as.
    pub(super) highlight: Option<&'a str>,
}

/// `markdown` as lines at most `width` columns wide. Code lines are not
/// wrapped.
pub(super) fn render(
    markdown: &str,
    code: &Code<'_>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut out = Writer {
        lines: Vec::new(),
        gap: false,
        inline: Vec::new(),
        styles: vec![Style::default()],
        lists: Vec::new(),
        items: 0,
        fence: None,
        width: width.max(1),
        theme,
        code,
    };
    for event in Parser::new(markdown) {
        out.event(event);
    }
    out.flush();
    out.lines
}

/// A piece of inline text, or a forced line break.
enum Piece {
    Text(String, Style),
    Break,
}

struct Writer<'a> {
    lines: Vec<Line<'static>>,
    /// A blank line goes before the next block.
    gap: bool,
    inline: Vec<Piece>,
    styles: Vec<Style>,
    /// The next number of each open list; `None` for bullets.
    lists: Vec<Option<u64>>,
    /// Open list items.
    items: usize,
    /// The open code block's language and text.
    fence: Option<(String, String)>,
    width: usize,
    theme: &'a Theme,
    code: &'a Code<'a>,
}

impl Writer<'_> {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.fence {
                Some((_, code)) => code.push_str(&text),
                None => self.text(&text, self.style()),
            },
            Event::Code(text) => {
                let style = self.style().fg(rgb_to_color(self.theme.border_active));
                self.text(&text, style);
            }
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => self.inline.push(Piece::Break),
            Event::Rule => {
                self.flush();
                self.gap = !self.lines.is_empty();
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph if self.items > 0 => {}
            Tag::Paragraph => self.block(),
            Tag::Heading { .. } => {
                self.block();
                self.push_style(Style::default().add_modifier(Modifier::BOLD));
            }
            Tag::CodeBlock(kind) => {
                self.block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.fence = Some((language, String::new()));
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.block();
                } else {
                    self.flush();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                self.items += 1;
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.text(&format!("{}{marker}", "  ".repeat(depth)), Style::default());
            }
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph if self.items > 0 => self.inline.push(Piece::Break),
            TagEnd::Paragraph => self.end_block(),
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.end_block();
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                self.items = self.items.saturating_sub(1);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn end_code_block(&mut self) {
        if let Some((language, code)) = self.fence.take() {
            let highlight = self.highlight(&language);
            let lines = render_code_block(&code, highlight, self.theme);
            self.lines.extend(lines.into_iter().map(Line::from));
        }
        self.gap = true;
    }

    /// The syntax a fence naming `language` is highlighted as: the hover's
    /// own for no language or its own; plain for any other.
    fn highlight(&self, language: &str) -> Option<&str> {
        let own = language.is_empty()
            || language.eq_ignore_ascii_case(self.code.language)
            || self
                .code
                .highlight
                .is_some_and(|highlight| language.eq_ignore_ascii_case(highlight));
        if own { self.code.highlight } else { None }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, style: Style) {
        let style = self.style().patch(style);
        self.styles.push(style);
    }

    fn text(&mut self, text: &str, style: Style) {
        self.inline.push(Piece::Text(text.to_string(), style));
    }

    /// Start a block: a blank line after the one before.
    fn block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.gap = false;
    }

    fn end_block(&mut self) {
        self.flush();
        self.gap = true;
    }

    /// Wrap the inline text gathered so far into lines.
    fn flush(&mut self) {
        let pieces = std::mem::take(&mut self.inline);
        if pieces.is_empty() {
            return;
        }
        let mut words = Words::default();
        let mut lines = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Break => lines.extend(wrap(words.take(), self.width)),
                Piece::Text(text, style) => words.add(&text, style),
            }
        }
        lines.extend(wrap(words.take(), self.width));
        while lines
            .last()
            .is_some_and(|line: &Line| line.spans.is_empty())
        {
            lines.pop();
        }
        self.lines.extend(lines);
    }
}

/// Inline text split into words: runs without whitespace, each made of
/// spans in their own styles.
#[derive(Default)]
struct Words {
    done: Vec<Vec<Span<'static>>>,
    word: Vec<Span<'static>>,
}

impl Words {
    fn add(&mut self, text: &str, style: Style) {
        let mut parts = text.split(char::is_whitespace).peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                self.word.push(Span::styled(part.to_string(), style));
            }
            if parts.peek().is_some() {
                self.end_word();
            }
        }
    }

    fn end_word(&mut self) {
        if !self.word.is_empty() {
            self.done.push(std::mem::take(&mut self.word));
        }
    }

    /// Every word so far, the last one included.
    fn take(&mut self) -> Vec<Vec<Span<'static>>> {
        self.end_word();
        std::mem::take(&mut self.done)
    }
}

/// `words` filled greedily into lines of at most `width` columns, one
/// space apart. A word wider than `width` gets a line of its own.
fn wrap(words: Vec<Vec<Span<'static>>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for word in words {
        let size: usize = word.iter().map(|span| display_width(&span.content)).sum();
        if !line.is_empty() && used + 1 + size > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            used = 0;
        }
        if !line.is_empty() {
            line.push(Span::raw(" "));
            used += 1;
        }
        used += size;
        line.extend(word);
    }
    lines.push(Line::from(line));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    const RUST: Code<'static> = Code {
        language: "Rust",
        highlight: Some("Rust"),
    };

    #[test]
    fn a_rust_analyzer_hover_reads_as_code_a_gap_and_wrapped_docs() {
        let markdown = "```rust\nshop::cart\n```\n\n```rust\npub fn total(items: &[u32]) -> u32\n```\n\n---\n\nAdds up every `item` in the cart, **including** taxes and fees.";
        let theme = Theme::default();
        let lines = render(markdown, &RUST, 30, &theme);
        assert_eq!(
            text(&lines),
            [
                "shop::cart",
                "",
                "pub fn total(items: &[u32]) -> u32",
                "",
                "Adds up every item in the",
                "cart, including taxes and",
                "fees.",
            ]
        );
        let colors: std::collections::HashSet<_> =
            lines[2].spans.iter().map(|span| span.style.fg).collect();
        assert!(colors.len() > 2, "the signature is highlighted");
        let bold = lines[5]
            .spans
            .iter()
            .find(|span| span.content == "including")
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn lists_breaks_and_foreign_fences_keep_their_shape() {
        let markdown =
            "Steps:\n\n- first\n- second\n\nline one  \nline two\n\n```python\nx = 1\n```";
        let theme = Theme::default();
        let lines = render(markdown, &RUST, 40, &theme);
        assert_eq!(
            text(&lines),
            [
                "Steps:",
                "",
                "• first",
                "• second",
                "",
                "line one",
                "line two",
                "",
                "x = 1",
            ]
        );
        assert_eq!(lines[8].spans.len(), 1, "a foreign fence stays plain");
    }
}
