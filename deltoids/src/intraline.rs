//! Delta-style within-line diff highlighting.
//!
//! Implements the within-line diff algorithm from delta to compute token-level
//! emphasis for paired minus/plus lines in a diff subhunk.
//!
//! Pipeline:
//! 1. Tokenize lines with `\w+` regex + grapheme separators
//! 2. Align token sequences (Needleman-Wunsch with delta's cost model)
//! 3. Greedily pair minus/plus lines within a subhunk
//! 4. Annotate paired lines with emph/non-emph sections

use regex::Regex;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// Alignment cost model (matches delta's align.rs)
// ---------------------------------------------------------------------------

const DELETION_COST: usize = 2;
const INSERTION_COST: usize = 2;
const INITIAL_MISMATCH_PENALTY: usize = 1;

/// Default max line distance for pairing (delta's `max-line-distance`).
const MAX_LINE_DISTANCE: f64 = 0.6;

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    NoOp,
    Deletion,
    Insertion,
}

// ---------------------------------------------------------------------------
// Tokenization
// ---------------------------------------------------------------------------

static WORD_REGEX: OnceLock<Regex> = OnceLock::new();

fn word_regex() -> &'static Regex {
    WORD_REGEX.get_or_init(|| Regex::new(r"\w+").expect("word regex should compile"))
}

/// Tokenize a line using delta's algorithm:
/// 1. Start with empty token `""`
/// 2. Find all `\w+` matches
/// 3. If the line starts with non-word text, push an extra `""`
/// 4. Emit separator text between matches as single-grapheme tokens
/// 5. Emit each regex match as one token
/// 6. If the line has no word matches but has content, push an extra `""`
pub fn tokenize<'a>(line: &'a str) -> Vec<&'a str> {
    let mut tokens: Vec<&'a str> = vec![""];
    let regex = word_regex();
    let mut offset = 0usize;

    for mat in regex.find_iter(line) {
        // Delta pushes an extra "" when the first match doesn't start at
        // position 0, signalling that separator text leads the line.
        if offset == 0 && mat.start() > 0 {
            tokens.push("");
        }
        // Emit separator graphemes between previous end and this match start.
        if mat.start() > offset {
            let separator = &line[offset..mat.start()];
            for grapheme in separator.graphemes(true) {
                tokens.push(grapheme);
            }
        }
        tokens.push(mat.as_str());
        offset = mat.end();
    }

    // Trailing text after the last match (or the entire line if no matches).
    if offset < line.len() {
        // Delta pushes an extra "" when there were no word matches at all.
        if offset == 0 {
            tokens.push("");
        }
        for grapheme in line[offset..].graphemes(true) {
            tokens.push(grapheme);
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Alignment (Needleman-Wunsch with delta's cost model)
// ---------------------------------------------------------------------------

fn diagonal_match_cost(x: &[&str], y: &[&str], i: usize, j: usize, table: &[Vec<usize>]) -> usize {
    if x[i - 1] == y[j - 1] {
        table[i - 1][j - 1]
    } else {
        usize::MAX
    }
}

#[derive(Debug, Clone)]
struct Alignment {
    x_tokens: Vec<String>,
    y_tokens: Vec<String>,
    #[allow(dead_code)]
    table: Vec<Vec<usize>>,
    ops: Vec<Vec<Operation>>,
}

impl Alignment {
    /// Build an alignment between two token sequences.
    ///
    /// Matches delta's cost model exactly:
    /// - Diagonal (match) only when tokens are equal, cost = parent cost
    /// - Diagonal disabled (MAX) when tokens differ
    /// - Insertion/Deletion cost = parent cost + basic cost + penalty
    ///   where penalty = INITIAL_MISMATCH_PENALTY if parent was NoOp
    /// - First row/column include the initial mismatch penalty
    /// - Tie-breaking order: insertion, deletion, match
    pub fn new(x: &[&str], y: &[&str]) -> Self {
        let m = x.len();
        let n = y.len();
        let mut table = vec![vec![0usize; n + 1]; m + 1];
        let mut ops = vec![vec![Operation::NoOp; n + 1]; m + 1];

        // Initialize first column (deletions). Delta adds the penalty once.
        for i in 1..=m {
            table[i][0] = i * DELETION_COST + INITIAL_MISMATCH_PENALTY;
            ops[i][0] = Operation::Deletion;
        }
        // Initialize first row (insertions). Delta adds the penalty once.
        for j in 1..=n {
            table[0][j] = j * INSERTION_COST + INITIAL_MISMATCH_PENALTY;
            ops[0][j] = Operation::Insertion;
        }

        // Fill the table.
        for i in 1..=m {
            for j in 1..=n {
                // Insertion: move from (i, j-1) by consuming y[j-1].
                let ins_cost = insertion_cost(table[i][j - 1], ops[i][j - 1]);
                // Deletion: move from (i-1, j) by consuming x[i-1].
                let del_cost = deletion_cost(table[i - 1][j], ops[i - 1][j]);
                // Match: diagonal from (i-1, j-1), only if tokens equal.
                let match_cost = diagonal_match_cost(x, y, i, j, &table);

                // Tie-breaking: insertion > deletion > match.
                let candidates = [
                    (ins_cost, Operation::Insertion),
                    (del_cost, Operation::Deletion),
                    (match_cost, Operation::NoOp),
                ];

                let (cost, op) = candidates
                    .iter()
                    .copied()
                    .min_by_key(|(cost, _)| *cost)
                    .unwrap();

                table[i][j] = cost;
                ops[i][j] = op;
            }
        }

        Alignment {
            x_tokens: x.iter().map(|s| s.to_string()).collect(),
            y_tokens: y.iter().map(|s| s.to_string()).collect(),
            table,
            ops,
        }
    }

    /// Extract the sequence of operations by tracing back through the table.
    /// Since diagonal is only used for matches (no substitution), each
    /// NoOp in the table is a true match.
    pub fn operations(&self) -> Vec<Operation> {
        let mut result = Vec::new();
        let mut i = self.x_tokens.len();
        let mut j = self.y_tokens.len();

        while i > 0 || j > 0 {
            let op = self.ops[i][j];
            match op {
                Operation::Insertion => {
                    result.push(Operation::Insertion);
                    j -= 1;
                }
                Operation::Deletion => {
                    result.push(Operation::Deletion);
                    i -= 1;
                }
                Operation::NoOp => {
                    result.push(Operation::NoOp);
                    i -= 1;
                    j -= 1;
                }
            }
        }

        result.reverse();
        result
    }

    /// Run-length encode the operations.
    #[cfg(test)]
    fn coalesced_operations(&self) -> Vec<(Operation, usize)> {
        run_length_encode(&self.operations())
    }
}

#[cfg(test)]
fn run_length_encode(ops: &[Operation]) -> Vec<(Operation, usize)> {
    let mut result = Vec::new();
    for &op in ops {
        if let Some(last) = result.last_mut() {
            let (last_op, count): &mut (Operation, usize) = last;
            if *last_op == op {
                *count += 1;
                continue;
            }
        }
        result.push((op, 1));
    }
    result
}

/// Cost of an insertion following a cell with the given cost and operation.
fn insertion_cost(parent_cost: usize, parent_op: Operation) -> usize {
    parent_cost
        + INSERTION_COST
        + if parent_op == Operation::NoOp {
            INITIAL_MISMATCH_PENALTY
        } else {
            0
        }
}

/// Cost of a deletion following a cell with the given cost and operation.
fn deletion_cost(parent_cost: usize, parent_op: Operation) -> usize {
    parent_cost
        + DELETION_COST
        + if parent_op == Operation::NoOp {
            INITIAL_MISMATCH_PENALTY
        } else {
            0
        }
}

// ---------------------------------------------------------------------------
// Annotation
// ---------------------------------------------------------------------------

/// A section of an annotated line with its operation and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedSection {
    pub op: Operation,
    pub text: String,
}

/// Result of annotating a minus/plus line pair.
#[derive(Debug, Clone)]
pub struct AnnotationResult {
    pub minus_sections: Vec<AnnotatedSection>,
    pub plus_sections: Vec<AnnotatedSection>,
    pub distance: f64,
}

/// Annotate a minus and plus line pair.
/// Returns sections for each line and the normalized edit distance.
///
/// Matches delta's `annotate` function: processes coalesced (run-length
/// encoded) operations, producing one section per run. Then applies
/// whitespace coalescing.
pub fn annotate(minus_line: &str, plus_line: &str) -> AnnotationResult {
    let minus_tokens = tokenize(minus_line);
    let plus_tokens = tokenize(plus_line);
    let alignment = Alignment::new(&minus_tokens, &plus_tokens);
    // Use raw operations (not coalesced) to build sections, then
    // coalesce via run_length_encode like delta does.
    let ops = alignment.operations();
    let coalesced = rle_operations(&ops);

    let mut minus_sections = Vec::new();
    let mut plus_sections = Vec::new();
    let mut d_numer = 0usize;
    let mut d_denom = 0usize;

    let mut x_idx = 0usize;
    let mut y_idx = 0usize;

    for &(op, count) in &coalesced {
        match op {
            Operation::NoOp => {
                // Concatenate `count` tokens from both sides into one section.
                let minus_text = concat_tokens(&minus_tokens, x_idx, count);

                let section_text_for_minus = &minus_line[byte_offset(&minus_tokens, x_idx)
                    ..byte_end(&minus_tokens, x_idx + count, minus_line)];
                let section_text_for_plus = &plus_line[byte_offset(&plus_tokens, y_idx)
                    ..byte_end(&plus_tokens, y_idx + count, plus_line)];
                let width = UnicodeWidthStr::width(minus_text.trim());
                d_denom += 2 * width;
                minus_sections.push(AnnotatedSection {
                    op: Operation::NoOp,
                    text: section_text_for_minus.to_string(),
                });
                // Delta splits plus-side NoOp sections that have
                // trailing whitespace into content + whitespace.
                if let Some(split) = split_trailing_whitespace(section_text_for_plus) {
                    plus_sections.push(AnnotatedSection {
                        op: Operation::NoOp,
                        text: split.0.to_string(),
                    });
                    plus_sections.push(AnnotatedSection {
                        op: Operation::NoOp,
                        text: split.1.to_string(),
                    });
                } else {
                    plus_sections.push(AnnotatedSection {
                        op: Operation::NoOp,
                        text: section_text_for_plus.to_string(),
                    });
                }
                x_idx += count;
                y_idx += count;
            }
            Operation::Deletion => {
                let section_text = &minus_line[byte_offset(&minus_tokens, x_idx)
                    ..byte_end(&minus_tokens, x_idx + count, minus_line)];
                let width = UnicodeWidthStr::width(section_text.trim());
                d_numer += width;
                d_denom += width;
                minus_sections.push(AnnotatedSection {
                    op: Operation::Deletion,
                    text: section_text.to_string(),
                });
                x_idx += count;
            }
            Operation::Insertion => {
                let section_text = &plus_line[byte_offset(&plus_tokens, y_idx)
                    ..byte_end(&plus_tokens, y_idx + count, plus_line)];
                let width = UnicodeWidthStr::width(section_text.trim());
                d_numer += width;
                d_denom += width;
                plus_sections.push(AnnotatedSection {
                    op: Operation::Insertion,
                    text: section_text.to_string(),
                });
                y_idx += count;
            }
        }
    }

    let distance = if d_denom == 0 {
        0.0
    } else {
        d_numer as f64 / d_denom as f64
    };

    // Apply whitespace coalescing.
    let minus_sections = coalesce_whitespace(minus_sections);
    let plus_sections = coalesce_whitespace(plus_sections);

    AnnotationResult {
        minus_sections,
        plus_sections,
        distance,
    }
}

/// Run-length encode operations (same as delta's coalesced_operations).
fn rle_operations(ops: &[Operation]) -> Vec<(Operation, usize)> {
    let mut result = Vec::new();
    for &op in ops {
        if let Some((last_op, count)) = result.last_mut()
            && *last_op == op
        {
            *count += 1;
            continue;
        }
        result.push((op, 1));
    }
    result
}

/// Compute the byte offset of the start of the token at index `idx`.
fn byte_offset(tokens: &[&str], idx: usize) -> usize {
    tokens[..idx].iter().map(|t| t.len()).sum()
}

/// Compute the byte end of the token at index `idx + count - 1`, clamped
/// to line length.
fn byte_end(tokens: &[&str], end_idx: usize, line: &str) -> usize {
    let offset: usize = tokens[..end_idx].iter().map(|t| t.len()).sum();
    offset.min(line.len())
}

/// Concatenate `count` tokens starting at `start`.
fn concat_tokens(tokens: &[&str], start: usize, count: usize) -> String {
    tokens[start..start + count].iter().copied().collect()
}

/// Split a string into (content, trailing_whitespace) if it has non-empty
/// content followed by trailing whitespace.
///
/// Mirrors delta's `get_contents_before_trailing_whitespace`: only splits
/// when the trimmed content is non-empty and differs from the newline-
/// trimmed version.
fn split_trailing_whitespace(s: &str) -> Option<(&str, &str)> {
    let content = s.trim_end();
    if !content.is_empty() && content != s.trim_end_matches('\n') {
        Some((content, &s[content.len()..]))
    } else {
        None
    }
}

fn should_absorb_whitespace(
    previous: Option<&AnnotatedSection>,
    current: &AnnotatedSection,
    has_next: bool,
) -> bool {
    let is_whitespace_noop =
        current.op == Operation::NoOp && current.text.trim().is_empty() && !current.text.is_empty();

    is_whitespace_noop && has_next && previous.is_some_and(|section| section.op != Operation::NoOp)
}

/// Coalesce whitespace-only NoOp sections with the preceding change section.
///
/// Delta absorbs a whitespace-only NoOp into the previous operation when:
/// - The previous operation was a change (Deletion/Insertion) and there are
///   more sections after this one.
/// - The previous operation was NoOp (no visual effect, since the label
///   stays the same).
///
/// The meaningful case is the first one: whitespace between a change and
/// whatever follows gets the change highlight, producing smoother output
/// without tiny unhighlighted gaps.
fn coalesce_whitespace(sections: Vec<AnnotatedSection>) -> Vec<AnnotatedSection> {
    let mut result: Vec<AnnotatedSection> = Vec::new();

    for (i, section) in sections.iter().enumerate() {
        if should_absorb_whitespace(result.last(), section, i + 1 < sections.len()) {
            // Absorb into the previous change section.
            if let Some(last) = result.last_mut() {
                last.text.push_str(&section.text);
                continue;
            }
        }

        result.push(section.clone());
    }

    result
}

// ---------------------------------------------------------------------------
// Subhunk line pairing
// ---------------------------------------------------------------------------

/// Emphasis kind for a section of a rendered line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmphKind {
    /// Changed token in a paired line (strong background).
    Emph,
    /// Unchanged token in a paired line (weak background).
    NonEmph,
}

/// A section of a line with its emphasis kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmphSection {
    pub kind: EmphKind,
    pub text: String,
}

/// Per-line emphasis information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEmphasis {
    /// Line is unpaired; render with flat background.
    Plain,
    /// Line is paired; render with emph/non-emph sections.
    Paired(Vec<EmphSection>),
}

/// Compute emphasis for all lines in a subhunk.
///
/// Takes minus lines and plus lines (without the leading `-`/`+` prefix).
/// Returns emphasis info for each minus line and each plus line, in order.
#[allow(clippy::mut_range_bound)] // plus_cursor mutation is for next outer-loop iteration
pub fn compute_subhunk_emphasis(
    minus_lines: &[&str],
    plus_lines: &[&str],
) -> (Vec<LineEmphasis>, Vec<LineEmphasis>) {
    let mut minus_emphasis: Vec<LineEmphasis> = vec![LineEmphasis::Plain; minus_lines.len()];
    let mut plus_emphasis: Vec<LineEmphasis> = vec![LineEmphasis::Plain; plus_lines.len()];

    // Greedy forward pairing (matches delta's infer_edits).
    let mut plus_cursor = 0usize;

    for (mi, minus_line) in minus_lines.iter().enumerate() {
        let mut found = false;
        for pi in plus_cursor..plus_lines.len() {
            let result = annotate(minus_line, plus_lines[pi]);
            if result.distance <= MAX_LINE_DISTANCE {
                // Accept this pair.
                minus_emphasis[mi] = sections_to_emphasis(&result.minus_sections);
                plus_emphasis[pi] = sections_to_emphasis(&result.plus_sections);
                plus_cursor = pi + 1;
                found = true;
                break;
            }
        }
        if !found {
            minus_emphasis[mi] = LineEmphasis::Plain;
        }
    }

    (minus_emphasis, plus_emphasis)
}

fn sections_to_emphasis(sections: &[AnnotatedSection]) -> LineEmphasis {
    // Check if there are any change sections. If everything is NoOp,
    // still mark as paired but all NonEmph.
    let emph_sections: Vec<EmphSection> = sections
        .iter()
        .map(|s| EmphSection {
            kind: match s.op {
                Operation::NoOp => EmphKind::NonEmph,
                Operation::Deletion | Operation::Insertion => EmphKind::Emph,
            },
            text: s.text.clone(),
        })
        .collect();

    LineEmphasis::Paired(emph_sections)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Tokenization tests (ported from delta's src/edits.rs)
    // -----------------------------------------------------------------------

    /// Delta's assert_tokenize strips the leading "". We check the full
    /// token stream here.
    fn assert_tokenize(text: &str, expected_without_leading_empty: &[&str]) {
        let actual = tokenize(text);
        assert_eq!(actual[0], "", "first token must be \"\"");
        assert_eq!(&actual[1..], expected_without_leading_empty);
        // Concatenation of expected (without leading "") must reconstruct the line.
        let reconstructed: String = expected_without_leading_empty.iter().copied().collect();
        assert_eq!(reconstructed, text);
    }

    #[test]
    fn tokenize_empty_string() {
        // test_tokenize_0: empty string
        assert_eq!(tokenize(""), vec![""]);
    }

    #[test]
    fn tokenize_separator_only() {
        // test_tokenize_0 continued
        assert_tokenize(";", &["", ";"]);
        assert_tokenize(";;", &["", ";", ";"]);
        assert_tokenize(";;a", &["", ";", ";", "a"]);
        assert_tokenize(";;ab", &["", ";", ";", "ab"]);
        assert_tokenize(";;ab;", &["", ";", ";", "ab", ";"]);
        assert_tokenize(";;ab;;", &["", ";", ";", "ab", ";", ";"]);
    }

    #[test]
    fn tokenize_single_word() {
        // test_tokenize_1
        assert_tokenize("aaa bbb", &["aaa", " ", "bbb"]);
    }

    #[test]
    fn tokenize_words_and_separators() {
        // test_tokenize_2
        assert_tokenize(
            "fn coalesce_edits<'a, EditOperation>(",
            &[
                "fn",
                " ",
                "coalesce_edits",
                "<",
                "'",
                "a",
                ",",
                " ",
                "EditOperation",
                ">",
                "(",
            ],
        );
    }

    #[test]
    fn tokenize_with_extra_type_param() {
        // test_tokenize_3
        assert_tokenize(
            "fn coalesce_edits<'a, 'b, EditOperation>(",
            &[
                "fn",
                " ",
                "coalesce_edits",
                "<",
                "'",
                "a",
                ",",
                " ",
                "'",
                "b",
                ",",
                " ",
                "EditOperation",
                ">",
                "(",
            ],
        );
    }

    #[test]
    fn tokenize_method_call() {
        // test_tokenize_4
        assert_tokenize(
            "annotated_plus_lines.push(vec![(noop_insertion, plus_line)]);",
            &[
                "annotated_plus_lines",
                ".",
                "push",
                "(",
                "vec",
                "!",
                "[",
                "(",
                "noop_insertion",
                ",",
                " ",
                "plus_line",
                ")",
                "]",
                ")",
                ";",
            ],
        );
    }

    #[test]
    fn tokenize_leading_spaces() {
        // test_tokenize_5
        assert_tokenize(
            "         let col = Color::from_str(s).unwrap_or_else(|_| die());",
            &[
                "",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                "let",
                " ",
                "col",
                " ",
                "=",
                " ",
                "Color",
                ":",
                ":",
                "from_str",
                "(",
                "s",
                ")",
                ".",
                "unwrap_or_else",
                "(",
                "|",
                "_",
                "|",
                " ",
                "die",
                "(",
                ")",
                ")",
                ";",
            ],
        );
    }

    #[test]
    fn tokenize_unicode_arrow() {
        // test_tokenize_6
        assert_tokenize(
            "         (minus_file, plus_file) => format!(\"renamed: {} ⟶  {}\", minus_file, plus_file),",
            &[
                "",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                " ",
                "(",
                "minus_file",
                ",",
                " ",
                "plus_file",
                ")",
                " ",
                "=",
                ">",
                " ",
                "format",
                "!",
                "(",
                "\"",
                "renamed",
                ":",
                " ",
                "{",
                "}",
                " ",
                "⟶",
                " ",
                " ",
                "{",
                "}",
                "\"",
                ",",
                " ",
                "minus_file",
                ",",
                " ",
                "plus_file",
                ")",
                ",",
            ],
        );
    }

    #[test]
    fn tokenize_preserves_concatenation() {
        let line = "fn foo<'a>(x: &'a str) -> bool {";
        let tokens = tokenize(line);
        let reconstructed: String = tokens.iter().copied().collect();
        assert_eq!(reconstructed, line);
    }

    // -----------------------------------------------------------------------
    // Alignment tests (ported from delta's src/align.rs)
    // -----------------------------------------------------------------------

    #[test]
    fn run_length_encode_basic() {
        // test_run_length_encode
        use Operation::*;
        let ops = vec![Deletion, Deletion, NoOp, Insertion, Insertion, Insertion];
        let rle = run_length_encode(&ops);
        assert_eq!(rle, vec![(Deletion, 2), (NoOp, 1), (Insertion, 3)]);
    }

    #[test]
    fn align_aaa_to_aba() {
        // test_0: "aaa" -> "aba"
        let x = vec!["a", "a", "a"];
        let y = vec!["a", "b", "a"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::NoOp, 1),
                (Operation::Deletion, 1),
                (Operation::Insertion, 1),
                (Operation::NoOp, 1),
            ]
        );
    }

    #[test]
    fn align_nonascii() {
        // test_0_nonascii
        let x = vec!["á", "á", "á"];
        let y = vec!["á", "β", "á"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::NoOp, 1),
                (Operation::Deletion, 1),
                (Operation::Insertion, 1),
                (Operation::NoOp, 1),
            ]
        );
    }

    #[test]
    fn align_kitten_to_sitting() {
        // test_1: kitten -> sitting
        let x = vec!["k", "i", "t", "t", "e", "n"];
        let y = vec!["s", "i", "t", "t", "i", "n", "g"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        // k->s (del+ins), i-t-t match, e->i (del+ins), n match, g insertion
        assert_eq!(
            ops,
            vec![
                (Operation::Deletion, 1),
                (Operation::Insertion, 1),
                (Operation::NoOp, 3),
                (Operation::Deletion, 1),
                (Operation::Insertion, 1),
                (Operation::NoOp, 1),
                (Operation::Insertion, 1),
            ]
        );
    }

    #[test]
    fn align_saturday_to_sunday() {
        // test_2: saturday -> sunday
        let x = vec!["s", "a", "t", "u", "r", "d", "a", "y"];
        let y = vec!["s", "u", "n", "d", "a", "y"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::NoOp, 1),      // s
                (Operation::Deletion, 2),  // a, t
                (Operation::NoOp, 1),      // u
                (Operation::Deletion, 1),  // r
                (Operation::Insertion, 1), // n
                (Operation::NoOp, 3),      // d, a, y
            ]
        );
    }

    #[test]
    fn align_prefers_grouped_changes() {
        // test_3: prefer deletion-noop-insertion over fragmented.
        // Deletions grouped before the matching token, insertions after.
        let x = vec!["a", "b", "c"];
        let y = vec!["c", "b", "a"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::Deletion, 2),  // a, b deleted
                (Operation::NoOp, 1),      // c matches c
                (Operation::Insertion, 2), // b, a inserted
            ]
        );
    }

    #[test]
    fn align_deletions_grouped() {
        // test_4
        let x = vec!["a", "b", "c", "d"];
        let y = vec!["a", "d"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::NoOp, 1),     // a
                (Operation::Deletion, 2), // b, c
                (Operation::NoOp, 1),     // d
            ]
        );
    }

    #[test]
    fn align_insertions_grouped() {
        // test_5
        let x = vec!["a", "d"];
        let y = vec!["a", "b", "c", "d"];
        let alignment = Alignment::new(&x, &y);
        let ops = alignment.coalesced_operations();
        assert_eq!(
            ops,
            vec![
                (Operation::NoOp, 1),      // a
                (Operation::Insertion, 2), // b, c
                (Operation::NoOp, 1),      // d
            ]
        );
    }

    // -----------------------------------------------------------------------
    // Annotation and distance tests
    // -----------------------------------------------------------------------

    #[test]
    fn annotate_identical_lines() {
        let result = annotate("hello world", "hello world");
        assert_eq!(result.distance, 0.0);
        // All sections should be NoOp.
        for section in &result.minus_sections {
            assert_eq!(section.op, Operation::NoOp);
        }
        for section in &result.plus_sections {
            assert_eq!(section.op, Operation::NoOp);
        }
    }

    #[test]
    fn annotate_single_word_change() {
        let result = annotate("const x = 1;", "const x = 2;");
        assert!(result.distance < MAX_LINE_DISTANCE);

        // The minus side should have a Deletion for "1".
        let has_deletion = result
            .minus_sections
            .iter()
            .any(|s| s.op == Operation::Deletion && s.text.contains('1'));
        assert!(
            has_deletion,
            "expected deletion of '1': {:?}",
            result.minus_sections
        );

        // The plus side should have an Insertion for "2".
        let has_insertion = result
            .plus_sections
            .iter()
            .any(|s| s.op == Operation::Insertion && s.text.contains('2'));
        assert!(
            has_insertion,
            "expected insertion of '2': {:?}",
            result.plus_sections
        );
    }

    #[test]
    fn annotate_completely_different_lines() {
        let result = annotate("aaa bbb ccc", "xxx yyy zzz");
        // All content changed, distance should be 1.0.
        assert!(
            result.distance > MAX_LINE_DISTANCE,
            "expected distance > {}, got {}",
            MAX_LINE_DISTANCE,
            result.distance
        );
    }

    #[test]
    fn annotate_sections_reconstruct_original() {
        let minus = "fn foo(x: i32) -> bool {";
        let plus = "fn bar(x: i32) -> bool {";
        let result = annotate(minus, plus);

        let minus_reconstructed: String = result
            .minus_sections
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        let plus_reconstructed: String = result
            .plus_sections
            .iter()
            .map(|s| s.text.as_str())
            .collect();

        assert_eq!(minus_reconstructed, minus);
        assert_eq!(plus_reconstructed, plus);
    }

    #[test]
    fn distance_formula_basic() {
        // Simple case: "a" -> "b"
        // Deletion of "a" (width 1) + insertion of "b" (width 1)
        // d_numer = 1 + 1 = 2, d_denom = 1 + 1 = 2
        // distance = 1.0
        let result = annotate("a", "b");
        assert!((result.distance - 1.0).abs() < 0.01);
    }

    #[test]
    fn distance_formula_half_changed() {
        let result = annotate("a b", "a c");
        // Tokens: ["", "a", " ", "b"] and ["", "a", " ", "c"]
        // Alignment: NoOp(3), Del(1), Ins(1)
        // Distance (single-pass over coalesced ops, matching delta):
        //   NoOp(3): text="a ", trim="a", width=1, d_denom += 2
        //   Del(1): text="b", width=1, d_numer += 1, d_denom += 1
        //   Ins(1): text="c", width=1, d_numer += 1, d_denom += 1
        // d_numer=2, d_denom=4, distance = 0.5
        assert!(
            (result.distance - 0.5).abs() < 0.01,
            "got {}",
            result.distance
        );
    }

    // -----------------------------------------------------------------------
    // Subhunk pairing tests
    // -----------------------------------------------------------------------

    #[test]
    fn pair_similar_lines() {
        let minus = vec!["const x = 1;"];
        let plus = vec!["const x = 2;"];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);
        assert!(matches!(me[0], LineEmphasis::Paired(_)));
        assert!(matches!(pe[0], LineEmphasis::Paired(_)));
    }

    #[test]
    fn no_pair_for_dissimilar_lines() {
        let minus = vec!["aaa bbb ccc ddd"];
        let plus = vec!["xxx yyy zzz www"];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);
        assert_eq!(me[0], LineEmphasis::Plain);
        assert_eq!(pe[0], LineEmphasis::Plain);
    }

    #[test]
    fn greedy_forward_pairing() {
        // minus[0] pairs with plus[1], minus[1] should pair with plus[2].
        // plus[0] stays unpaired because greedy goes forward.
        let minus = vec!["let a = 1;", "let b = 2;"];
        let plus = vec![
            "something completely different xxxxxx",
            "let a = 10;",
            "let b = 20;",
        ];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);

        // minus[0] should pair with plus[1] (similar)
        assert!(matches!(me[0], LineEmphasis::Paired(_)));
        assert!(matches!(me[1], LineEmphasis::Paired(_)));

        // plus[0] should be unpaired (dissimilar to both minus lines)
        assert_eq!(pe[0], LineEmphasis::Plain);
        // plus[1] and plus[2] should be paired
        assert!(matches!(pe[1], LineEmphasis::Paired(_)));
        assert!(matches!(pe[2], LineEmphasis::Paired(_)));
    }

    #[test]
    fn surplus_minus_lines_unpaired() {
        let minus = vec!["let a = 1;", "let b = 2;", "let c = 3;"];
        let plus = vec!["let a = 10;"];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);

        assert!(matches!(me[0], LineEmphasis::Paired(_)));
        assert_eq!(me[1], LineEmphasis::Plain);
        assert_eq!(me[2], LineEmphasis::Plain);
        assert!(matches!(pe[0], LineEmphasis::Paired(_)));
    }

    #[test]
    fn surplus_plus_lines_unpaired() {
        let minus = vec!["let a = 1;"];
        let plus = vec!["let a = 10;", "let b = 20;", "let c = 30;"];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);

        assert!(matches!(me[0], LineEmphasis::Paired(_)));
        assert!(matches!(pe[0], LineEmphasis::Paired(_)));
        assert_eq!(pe[1], LineEmphasis::Plain);
        assert_eq!(pe[2], LineEmphasis::Plain);
    }

    #[test]
    fn empty_subhunk() {
        let (me, pe) = compute_subhunk_emphasis(&[], &[]);
        assert!(me.is_empty());
        assert!(pe.is_empty());
    }

    #[test]
    fn paired_line_has_emph_and_non_emph() {
        let minus = vec!["const x = 1;"];
        let plus = vec!["const x = 2;"];
        let (me, pe) = compute_subhunk_emphasis(&minus, &plus);

        if let LineEmphasis::Paired(ref sections) = me[0] {
            let has_emph = sections.iter().any(|s| s.kind == EmphKind::Emph);
            let has_non_emph = sections.iter().any(|s| s.kind == EmphKind::NonEmph);
            assert!(has_emph, "paired minus should have emph sections");
            assert!(has_non_emph, "paired minus should have non-emph sections");
        } else {
            panic!("expected paired emphasis for minus line");
        }

        if let LineEmphasis::Paired(ref sections) = pe[0] {
            let has_emph = sections.iter().any(|s| s.kind == EmphKind::Emph);
            let has_non_emph = sections.iter().any(|s| s.kind == EmphKind::NonEmph);
            assert!(has_emph, "paired plus should have emph sections");
            assert!(has_non_emph, "paired plus should have non-emph sections");
        } else {
            panic!("expected paired emphasis for plus line");
        }
    }

    #[test]
    fn paired_sections_reconstruct_line() {
        let minus = "fn foo(x: i32) -> bool {";
        let plus = "fn bar(x: i32) -> bool {";
        let (me, pe) = compute_subhunk_emphasis(&[minus], &[plus]);

        if let LineEmphasis::Paired(ref sections) = me[0] {
            let reconstructed: String = sections.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(reconstructed, minus);
        }
        if let LineEmphasis::Paired(ref sections) = pe[0] {
            let reconstructed: String = sections.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(reconstructed, plus);
        }
    }

    // -------------------------------------------------------------------
    // Edit inference tests (ported from delta's src/edits.rs)
    //
    // Delta's output uses (MinusNoop, "..."), (Deletion, "..."), etc.
    // We map those to our AnnotatedSection with Operation::{NoOp, Deletion, Insertion}.
    // -------------------------------------------------------------------

    /// Assert that annotating a pair produces the expected sections.
    /// `expected_minus` and `expected_plus` are slices of (Operation, &str).
    fn assert_annotation(
        minus_line: &str,
        plus_line: &str,
        expected_minus: &[(Operation, &str)],
        expected_plus: &[(Operation, &str)],
    ) {
        let result = annotate(minus_line, plus_line);

        let actual_minus: Vec<(Operation, &str)> = result
            .minus_sections
            .iter()
            .map(|s| (s.op, s.text.as_str()))
            .collect();
        let actual_plus: Vec<(Operation, &str)> = result
            .plus_sections
            .iter()
            .map(|s| (s.op, s.text.as_str()))
            .collect();

        assert_eq!(
            actual_minus, expected_minus,
            "minus mismatch for {:?} -> {:?}",
            minus_line, plus_line
        );
        assert_eq!(
            actual_plus, expected_plus,
            "plus mismatch for {:?} -> {:?}",
            minus_line, plus_line
        );

        // Verify reconstruction.
        let minus_reconstructed: String = result
            .minus_sections
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        let plus_reconstructed: String = result
            .plus_sections
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(minus_reconstructed, minus_line);
        assert_eq!(plus_reconstructed, plus_line);
    }

    #[test]
    fn diagonal_match_cost_uses_parent_cost_for_equal_tokens() {
        let table = vec![vec![0, 0], vec![0, 7]];
        assert_eq!(diagonal_match_cost(&["a"], &["a"], 1, 1, &table), 0);
    }

    #[test]
    fn infer_edits_1_whole_word_change() {
        // test_infer_edits_1: "aaa" -> "aba"
        assert_annotation(
            "aaa",
            "aba",
            &[(Operation::NoOp, ""), (Operation::Deletion, "aaa")],
            &[(Operation::NoOp, ""), (Operation::Insertion, "aba")],
        );
    }

    #[test]
    fn infer_edits_1_2_partial_match() {
        // test_infer_edits_1_2: "aaa ccc" -> "aba ccc"
        assert_annotation(
            "aaa ccc",
            "aba ccc",
            &[
                (Operation::NoOp, ""),
                (Operation::Deletion, "aaa"),
                (Operation::NoOp, " ccc"),
            ],
            &[
                (Operation::NoOp, ""),
                (Operation::Insertion, "aba"),
                (Operation::NoOp, " ccc"),
            ],
        );
    }

    #[test]
    fn infer_edits_3_method_rename() {
        // test_infer_edits_3: "d.iteritems()" -> "d.items()"
        assert_annotation(
            "d.iteritems()",
            "d.items()",
            &[
                (Operation::NoOp, "d."),
                (Operation::Deletion, "iteritems"),
                (Operation::NoOp, "()"),
            ],
            &[
                (Operation::NoOp, "d."),
                (Operation::Insertion, "items"),
                (Operation::NoOp, "()"),
            ],
        );
    }

    #[test]
    fn infer_edits_7_added_type_param() {
        // test_infer_edits_7
        assert_annotation(
            "fn coalesce_edits<'a, EditOperation>(",
            "fn coalesce_edits<'a, 'b, EditOperation>(",
            &[
                (Operation::NoOp, "fn coalesce_edits<'a, "),
                (Operation::NoOp, "EditOperation>("),
            ],
            &[
                (Operation::NoOp, "fn coalesce_edits<'a,"),
                (Operation::NoOp, " "),
                (Operation::Insertion, "'b, "),
                (Operation::NoOp, "EditOperation>("),
            ],
        );
    }

    #[test]
    fn infer_edits_11_appended_word() {
        // test_infer_edits_11
        assert_annotation(
            "                 self.table[index] =",
            "                 self.table[index] = candidates",
            &[(Operation::NoOp, "                 self.table[index] =")],
            &[
                (Operation::NoOp, "                 self.table[index] ="),
                (Operation::Insertion, " candidates"),
            ],
        );
    }

    #[test]
    fn infer_edits_12_removed_word() {
        // test_infer_edits_12
        assert_annotation(
            r#"                     (xxxxxxxxx, "build info"),"#,
            r#"                     (xxxxxxxxx, "build"),"#,
            &[
                (
                    Operation::NoOp,
                    r#"                     (xxxxxxxxx, "build"#,
                ),
                (Operation::Deletion, " info"),
                (Operation::NoOp, r#""),"#),
            ],
            &[
                (
                    Operation::NoOp,
                    r#"                     (xxxxxxxxx, "build"#,
                ),
                (Operation::NoOp, r#""),"#),
            ],
        );
    }

    #[test]
    fn infer_edits_15_command_substitution() {
        // test_infer_edits_15
        assert_annotation(
            r#"printf "%s\n" s y y | git add -p &&"#,
            "test_write_lines s y y | git add -p &&",
            &[
                (Operation::NoOp, ""),
                (Operation::Deletion, r#"printf "%s\n""#),
                (Operation::NoOp, " s y y | git add -p &&"),
            ],
            &[
                (Operation::NoOp, ""),
                (Operation::Insertion, "test_write_lines"),
                (Operation::NoOp, " s y y | git add -p &&"),
            ],
        );
    }

    #[test]
    fn should_absorb_whitespace_after_change_when_more_sections_follow() {
        let previous = AnnotatedSection {
            op: Operation::Deletion,
            text: "value".to_string(),
        };
        let whitespace = AnnotatedSection {
            op: Operation::NoOp,
            text: " ".to_string(),
        };

        assert!(should_absorb_whitespace(Some(&previous), &whitespace, true));
    }
}
