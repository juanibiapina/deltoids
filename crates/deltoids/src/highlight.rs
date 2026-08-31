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
//! Lines are highlighted with an appended `\n` so end-of-line-terminated
//! contexts (line comments like `//`) pop on the newline token instead of
//! staying on syntect's stack and painting later lines. Both call sites that
//! feed a state — the new side and the discarded minus-side advance in
//! [`HunkHighlighter::context`] — go through [`highlight_with_eol`].
//!
//! Residual limitation: state is seeded fresh at the first line of each hunk.
//! If a hunk begins *inside* a multi-line construct (its opening delimiter is
//! not part of the hunk), the leading lines still mis-highlight. In practice
//! deltoids' context expansion pulls the whole leading comment/literal into
//! the hunk, so the opening delimiter is usually present.

use syntect::easy::HighlightLines;
use syntect::highlighting::Style;
use syntect::highlighting::Theme as SyntectTheme;
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
    /// name) to pick the grammar and `syntax_theme` (resolved through
    /// [`crate::theme_by_name`]) to pick the colors. Both sides start from the
    /// grammar's initial state.
    pub(crate) fn new(highlight: Option<&str>, syntax_theme: &'static SyntectTheme) -> Self {
        let assets = SyntaxAssets::load();
        let syntax = assets.syntax_for_name(highlight);
        Self {
            minus: HighlightLines::new(syntax, syntax_theme),
            plus: HighlightLines::new(syntax, syntax_theme),
            syntax_set: assets.syntax_set,
        }
    }

    /// Highlight a context line. Feeds it to both sides (keeping the old side
    /// in sync) and returns the ranges from the new side.
    pub(crate) fn context<'a>(&mut self, line: &'a str) -> Vec<(Style, &'a str)> {
        // Advance the minus side too so removed lines later in the hunk see
        // the correct old-side state; discard its ranges. Still append the
        // newline so line-comment contexts pop on this side as well.
        let _ = highlight_with_eol(&mut self.minus, line, self.syntax_set);
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
    match highlight_with_eol(state, line, syntax_set) {
        Ok(ranges) => ranges,
        Err(_) => vec![(Style::default(), line)],
    }
}

/// Highlight `line` with an appended `\n` and remap the resulting ranges back
/// onto the original `line`.
///
/// syntect pops end-of-line-terminated contexts (line comments) only when it
/// sees the newline token. Feeding a line without `\n` leaves that context on
/// the parse stack, so every later line in the hunk is painted with the
/// comment style. We highlight `line + "\n"`, then walk the ranges, clip the
/// trailing newline byte, and re-borrow slices of the caller's `line` so the
/// returned text reconstructs `line` byte-exact.
fn highlight_with_eol<'a>(
    state: &mut HighlightLines<'static>,
    line: &'a str,
    syntax_set: &SyntaxSet,
) -> Result<Vec<(Style, &'a str)>, syntect::Error> {
    let owned = format!("{line}\n");
    let ranges = state.highlight_line(&owned, syntax_set)?;

    let len = line.len();
    let mut offset = 0;
    let mut out = Vec::with_capacity(ranges.len());
    for (style, piece) in ranges {
        let start = offset.min(len);
        let end = (offset + piece.len()).min(len);
        offset += piece.len();
        if start < end {
            out.push((style, &line[start..end]));
        }
    }
    Ok(out)
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
        let mut hl =
            HunkHighlighter::new(Some("TypeScriptReact"), crate::config::theme_by_name(None));
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

    fn distinct_fgs(
        ranges: &[(Style, &str)],
    ) -> std::collections::HashSet<syntect::highlighting::Color> {
        ranges.iter().map(|(style, _)| style.foreground).collect()
    }

    /// C's grammar keeps its line-comment context on the stack until it sees a
    /// newline. A full-line `// comment` fed via `context` must not leak the
    /// comment color onto the following code line: the comment line stays
    /// uniform, and the code line after it fragments into multiple colors.
    #[test]
    fn context_line_comment_does_not_leak_to_next_line() {
        let mut hl = HunkHighlighter::new(Some("C"), crate::config::theme_by_name(None));
        hl.context("int a = 1;");
        let comment = hl.context("// a full line comment");
        let after = hl.context("int x = termios;");

        assert_eq!(
            distinct_fgs(&comment).len(),
            1,
            "the comment line should be one uniform comment color, got {:?}",
            distinct_fgs(&comment)
        );
        assert!(
            distinct_fgs(&after).len() > 1,
            "code after a line comment should highlight as code, got {:?}",
            distinct_fgs(&after)
        );
    }

    /// Guard for the minus-side advance in `context`: after a context line
    /// comment, a removed line must highlight as code. This fails if only the
    /// plus side appends the newline.
    #[test]
    fn removed_after_context_line_comment_does_not_leak() {
        let mut hl = HunkHighlighter::new(Some("C"), crate::config::theme_by_name(None));
        hl.context("int a;");
        hl.context("// a full line comment");
        let removed = hl.removed("int x = termios;");

        assert!(
            distinct_fgs(&removed).len() > 1,
            "removed line after a context line comment should highlight as \
             code, got {:?}",
            distinct_fgs(&removed)
        );
    }

    /// A trailing comment keeps the code before it colored and lets the next
    /// line recover.
    #[test]
    fn trailing_line_comment_does_not_leak() {
        let mut hl = HunkHighlighter::new(Some("C"), crate::config::theme_by_name(None));
        let trailing = hl.context("int a = 1; // note");
        let after = hl.context("int x = termios;");

        assert!(
            distinct_fgs(&trailing).len() > 1,
            "a line with code then a trailing comment should have multiple \
             colors, got {:?}",
            distinct_fgs(&trailing)
        );
        assert!(
            distinct_fgs(&after).len() > 1,
            "code after a trailing comment should highlight as code, got {:?}",
            distinct_fgs(&after)
        );
    }

    /// Without state carry, the same second line is highlighted from scratch
    /// and its code-looking words get distinct colors. This proves the carry
    /// is what fixes it.
    #[test]
    fn fresh_highlighter_miscolors_second_comment_line() {
        let mut fresh =
            HunkHighlighter::new(Some("TypeScriptReact"), crate::config::theme_by_name(None));
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
