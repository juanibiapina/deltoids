//! File-header badge that tells whether deltoids understands a file's
//! syntax. A language with tree-sitter scope support shows its glyph and
//! name; a file syntect highlights without scope support shows a muted
//! "no scope context" marker; plain text shows nothing.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use deltoids::render_tui::rgb_to_color;
use deltoids::{Language, Theme};

use crate::cli::browse::text::display_width;
use crate::sidebar::{IconMode, file_icon};

/// Syntect's syntax name for plain text, which has nothing to support.
const PLAIN_TEXT: &str = "Plain Text";

/// Columns kept free between the header text and the badge.
const GAP: usize = 1;

#[derive(Debug, Clone)]
pub(in crate::cli::browse) struct SyntaxBadge {
    spans: Vec<Span<'static>>,
    width: usize,
}

impl SyntaxBadge {
    /// The badge for a file with tree-sitter `language` and syntect
    /// `highlight` name at `path`. `None` for plain text (neither).
    pub(in crate::cli::browse) fn new(
        language: Option<Language>,
        highlight: Option<&str>,
        path: &str,
        icons: IconMode,
        theme: &Theme,
    ) -> Option<Self> {
        let muted = Style::default().fg(rgb_to_color(theme.muted));
        let spans = match (language, highlight) {
            (Some(language), _) => {
                let mut spans = Vec::new();
                if icons == IconMode::On {
                    let icon = file_icon(path);
                    spans.push(Span::styled(
                        format!("{} ", icon.glyph),
                        Style::default().fg(rgb_to_color(icon.color)),
                    ));
                }
                spans.push(Span::styled(language.name(), muted));
                spans
            }
            (None, Some(highlight)) if highlight != PLAIN_TEXT => {
                vec![Span::styled(
                    format!("{highlight} · no scope context"),
                    muted,
                )]
            }
            (None, _) => return None,
        };
        let width = spans.iter().map(|span| display_width(&span.content)).sum();
        Some(SyntaxBadge { spans, width })
    }

    /// `line` padded so the badge sits flush right within `width`, or
    /// `line` unchanged when the text and the badge do not fit side by
    /// side (the text takes priority).
    pub(in crate::cli::browse) fn attach(
        &self,
        mut line: Line<'static>,
        width: usize,
    ) -> Line<'static> {
        let used: usize = line
            .spans
            .iter()
            .map(|span| display_width(&span.content))
            .sum();
        if used + GAP + self.width > width {
            return line;
        }
        line.spans
            .push(Span::raw(" ".repeat(width - used - self.width)));
        line.spans.extend(self.spans.iter().cloned());
        line
    }
}

/// `line` with the badge for `language`/`highlight` attached, or `line`
/// unchanged for plain text.
pub(in crate::cli::browse) fn with_syntax_badge(
    line: Line<'static>,
    language: Option<Language>,
    highlight: Option<&str>,
    path: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    match SyntaxBadge::new(language, highlight, path, IconMode::from_env(), theme) {
        Some(badge) => badge.attach(line, width),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(
        language: Option<Language>,
        highlight: Option<&str>,
        icons: IconMode,
        width: usize,
    ) -> Line<'static> {
        let theme = Theme::default();
        let badge =
            SyntaxBadge::new(language, highlight, "src/Main.kt", icons, &theme).expect("badge");
        badge.attach(Line::from("src/Main.kt"), width)
    }

    #[test]
    fn supported_language_shows_glyph_and_name() {
        let line = header(Some(Language::Kotlin), Some("Kotlin"), IconMode::On, 40);
        let text = line.to_string();
        assert!(text.starts_with("src/Main.kt"), "{text:?}");
        assert!(text.ends_with("\u{e634} Kotlin"), "{text:?}");
        assert_eq!(display_width(&text), 40);
    }

    #[test]
    fn supported_language_without_icons_shows_name_only() {
        let text = header(Some(Language::Kotlin), Some("Kotlin"), IconMode::Off, 40).to_string();
        assert!(text.ends_with(" Kotlin"), "{text:?}");
        assert!(!text.contains('\u{e634}'), "{text:?}");
    }

    #[test]
    fn unsupported_syntax_shows_muted_marker() {
        let theme = Theme::default();
        let line = header(None, Some("Dockerfile"), IconMode::On, 60);
        let badge = line.spans.last().expect("badge span");
        assert_eq!(badge.content, "Dockerfile · no scope context");
        assert_eq!(badge.style.fg, Some(rgb_to_color(theme.muted)));
    }

    #[test]
    fn plain_text_has_no_badge() {
        let theme = Theme::default();
        assert!(SyntaxBadge::new(None, None, "notes", IconMode::On, &theme).is_none());
        assert!(
            SyntaxBadge::new(None, Some("Plain Text"), "notes.txt", IconMode::On, &theme).is_none()
        );
    }

    #[test]
    fn narrow_header_drops_the_badge() {
        let line = header(Some(Language::Kotlin), Some("Kotlin"), IconMode::Off, 17);
        assert_eq!(line.to_string(), "src/Main.kt");
    }
}
