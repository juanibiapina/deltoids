//! A pull request's header, above its commits' headers when the review's
//! root row is selected: what a reviewer reads before the diff. It reads
//! like a commit's `git show` header, with the title as the subject and
//! the description, rendered from Markdown, as the body.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use deltoids::Theme;
use deltoids::render_tui::rgb_to_color;

use super::hover::markdown_lines;
use crate::cli::browse::text::wrap_text;
use crate::pull_request::PullRequestInfo;

const INDENT: &str = "    ";

/// `info` as lines at most `width` columns wide, ending in a blank line.
pub(super) fn lines(info: &PullRequestInfo, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let accent = Style::default().fg(rgb_to_color(theme.status_added));
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let mut lines = vec![Line::from(vec![
        Span::raw("pull request "),
        Span::styled(format!("#{}", info.number), accent),
    ])];
    if !info.author.is_empty() {
        lines.push(Line::raw(format!("Author: {}", info.author)));
    }
    if !info.head.is_empty() && !info.base.is_empty() {
        lines.push(Line::raw(format!("Branch: {} → {}", info.head, info.base)));
    }
    if !info.url.is_empty() {
        lines.push(Line::raw(format!("URL:    {}", info.url)));
    }
    lines.push(Line::default());

    let room = width.saturating_sub(INDENT.len()).max(1);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    for part in wrap_text(&info.title, room) {
        lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(part, bold),
        ]));
    }
    lines.push(Line::default());

    let body = markdown_lines(&info.body, room, theme);
    if body.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled("No description provided.", muted),
        ]));
    }
    for mut line in body {
        if !line.spans.is_empty() {
            line.spans.insert(0, Span::raw(INDENT));
        }
        lines.push(line);
    }
    lines.push(Line::default());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(body: &str) -> PullRequestInfo {
        PullRequestInfo {
            number: 7,
            title: "Make a better".to_string(),
            author: "ada".to_string(),
            url: "https://github.com/acme/widgets/pull/7".to_string(),
            base: "main".to_string(),
            head: "ada:better-a".to_string(),
            body: body.to_string(),
        }
    }

    fn text(info: &PullRequestInfo, width: usize) -> Vec<String> {
        lines(info, width, &Theme::default())
            .iter()
            .map(Line::to_string)
            .collect()
    }

    #[test]
    fn reads_like_a_commit_header_with_the_title_as_subject() {
        assert_eq!(
            text(
                &info("Edits **a**.\n\n<!-- template -->\n\n- one\n- two\n"),
                80
            ),
            [
                "pull request #7",
                "Author: ada",
                "Branch: ada:better-a → main",
                "URL:    https://github.com/acme/widgets/pull/7",
                "",
                "    Make a better",
                "",
                "    Edits a.",
                "",
                "    • one",
                "    • two",
                "",
            ]
        );
    }

    #[test]
    fn says_when_there_is_no_description() {
        assert_eq!(text(&info(""), 80)[7], "    No description provided.");
    }

    #[test]
    fn wraps_the_description_under_the_indent() {
        let lines = text(&info("one two three four five six"), 18);
        assert_eq!(lines[7..9], ["    one two three", "    four five six"]);
    }
}
