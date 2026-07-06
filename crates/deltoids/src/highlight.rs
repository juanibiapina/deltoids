//! Stateful, two-sided syntax highlighter for a single hunk.
//!
//! syntect's [`HighlightLines`] is stateful: its parse/highlight state must
//! carry from one line to the next for multi-line scopes (block comments,
//! multi-line strings, template literals, heredocs) to stay in scope.
//! Highlighting each line with a fresh [`HighlightLines`] throws that state
//! away, so line 2+ of a `/** … */` block is re-tokenized as top-level code
//! and its words get code-token colors.
//!
//! A hunk shows two evolving versions of a file (old and new), so this holds
//! **two** highlighter states advanced independently:
//!
//! - `minus`: advanced by context lines + removed lines (the old side).
//! - `plus`: advanced by context lines + added lines (the new side).
//!
//! Context lines are identical on both sides, so they are fed to *both*
//! states (to keep them in lockstep) and rendered from the `plus` side.
//! Removed lines feed/render from `minus`; added lines feed/render from
//! `plus`. Feeding each body line as [`crate::Hunk::runs`] emits it preserves
//! the correct per-side sequence.
//!
//! Residual limitation: state is seeded fresh at the first line of each hunk.
//! If a hunk begins *inside* a multi-line construct (its opening delimiter is
//! not part of the hunk), the leading lines still mis-highlight. In practice
//! deltoids' context expansion pulls the whole leading comment/literal into
//! the hunk, so the opening delimiter is usually present.

use syntect::easy::HighlightLines;
use syntect::highlighting::Style;
use syntect::parsing::SyntaxSet;

use crate::config::SyntaxAssets;

/// Two-sided, stateful syntax highlighter scoped to one hunk.
///
/// Create one per hunk, then feed body lines in source order via
/// [`context`](Self::context), [`removed`](Self::removed), and
/// [`added`](Self::added). Each returns syntect ranges borrowing from the
/// passed line.
pub(crate) struct HunkHighlighter {
    minus: HighlightLines<'static>,
    plus: HighlightLines<'static>,
    syntax_set: &'static SyntaxSet,
}

impl HunkHighlighter {
    /// Build a highlighter for a hunk, using `highlight` (a syntect syntax
    /// name) to pick the grammar. Both sides start from the grammar's initial
    /// state.
    pub(crate) fn new(highlight: Option<&str>) -> Self {
        let assets = SyntaxAssets::load();
        let syntax = assets.syntax_for_name(highlight);
        Self {
            minus: HighlightLines::new(syntax, assets.syntax_theme),
            plus: HighlightLines::new(syntax, assets.syntax_theme),
            syntax_set: assets.syntax_set,
        }
    }

    /// Highlight a context line. Feeds it to both sides (keeping the old side
    /// in sync) and returns the ranges from the new side.
    pub(crate) fn context<'a>(&mut self, line: &'a str) -> Vec<(Style, &'a str)> {
        // Advance the minus side too so removed lines later in the hunk see
        // the correct old-side state; discard its ranges.
        let _ = self.minus.highlight_line(line, self.syntax_set);
        highlight_or_plain(&mut self.plus, line, self.syntax_set)
    }

    /// Highlight a removed (old-side) line.
    pub(crate) fn removed<'a>(&mut self, line: &'a str) -> Vec<(Style, &'a str)> {
        highlight_or_plain(&mut self.minus, line, self.syntax_set)
    }

    /// Highlight an added (new-side) line.
    pub(crate) fn added<'a>(&mut self, line: &'a str) -> Vec<(Style, &'a str)> {
        highlight_or_plain(&mut self.plus, line, self.syntax_set)
    }
}

/// Highlight one line through `state`, falling back to a single default-styled
/// range covering the whole line when syntect errors. The fallback mirrors the
/// renderers' previous plain-text behavior.
fn highlight_or_plain<'a>(
    state: &mut HighlightLines<'static>,
    line: &'a str,
    syntax_set: &SyntaxSet,
) -> Vec<(Style, &'a str)> {
    match state.highlight_line(line, syntax_set) {
        Ok(ranges) => ranges,
        Err(_) => vec![(Style::default(), line)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeding `/**` then a code-looking comment line: the second line's
    /// ranges must all carry the comment scope's style, not fresh code-token
    /// styles. We assert every range shares one foreground color and that it
    /// matches the first line's comment color.
    #[test]
    fn context_lines_carry_block_comment_scope() {
        let mut hl = HunkHighlighter::new(Some("TypeScriptReact"));
        let first = hl.context("/**");
        let second = hl.context(" * VAPID event fetch 404");

        // Reference comment color from the opener line.
        let comment_fg = first
            .iter()
            .map(|(style, _)| style.foreground)
            .next()
            .expect("opener produces at least one range");

        // Every range on the second line uses the same comment color; none of
        // the code-looking words get a distinct token color.
        let fgs: Vec<_> = second.iter().map(|(style, _)| style.foreground).collect();
        assert!(!fgs.is_empty(), "second comment line should produce ranges");
        for fg in &fgs {
            assert_eq!(
                *fg, comment_fg,
                "interior comment word got a non-comment color"
            );
        }
    }

    /// Without state carry, the same second line is highlighted from scratch
    /// and its code-looking words get distinct colors. This proves the carry
    /// is what fixes it.
    #[test]
    fn fresh_highlighter_miscolors_second_comment_line() {
        let mut fresh = HunkHighlighter::new(Some("TypeScriptReact"));
        let standalone = fresh.context(" * VAPID event fetch 404");
        let distinct: std::collections::HashSet<_> = standalone
            .iter()
            .map(|(style, _)| style.foreground)
            .collect();
        assert!(
            distinct.len() > 1,
            "a from-scratch highlight of the comment body should fragment \
             into multiple colors, got {distinct:?}"
        );
    }
}
