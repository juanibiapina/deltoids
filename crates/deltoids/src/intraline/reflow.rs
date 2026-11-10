use unicode_width::UnicodeWidthStr;

use super::{Alignment, EmphKind, EmphSection, LineEmphasis, Operation, tokenize};

const BLOCK_MAX_CELLS: usize = 250_000;

const REFLOW_MAX_DISTANCE: f64 = 0.2;

struct Token<'a> {
    line: usize,
    start: usize,
    text: &'a str,
}

pub(super) fn block_emphasis(
    minus_lines: &[&str],
    plus_lines: &[&str],
) -> Option<(Vec<LineEmphasis>, Vec<LineEmphasis>)> {
    if minus_lines.is_empty() || plus_lines.is_empty() {
        return None;
    }
    if minus_lines.len() == 1 && plus_lines.len() == 1 {
        return None;
    }

    let minus_tokens = content_tokens(minus_lines);
    let plus_tokens = content_tokens(plus_lines);
    let cells = (minus_tokens.len() + 1).checked_mul(plus_tokens.len() + 1)?;
    if cells > BLOCK_MAX_CELLS {
        return None;
    }

    let minus_texts: Vec<&str> = minus_tokens.iter().map(|t| t.text).collect();
    let plus_texts: Vec<&str> = plus_tokens.iter().map(|t| t.text).collect();
    let operations = Alignment::new(&minus_texts, &plus_texts).operations();

    let mut minus_changed = vec![false; minus_tokens.len()];
    let mut plus_changed = vec![false; plus_tokens.len()];
    let (mut i, mut j) = (0usize, 0usize);
    let (mut matched, mut changed) = (0usize, 0usize);
    for op in operations {
        match op {
            Operation::NoOp => {
                matched += minus_tokens[i].text.width();
                i += 1;
                j += 1;
            }
            Operation::Deletion => {
                minus_changed[i] = true;
                changed += minus_tokens[i].text.width();
                i += 1;
            }
            Operation::Insertion => {
                plus_changed[j] = true;
                changed += plus_tokens[j].text.width();
                j += 1;
            }
        }
    }

    let denominator = 2 * matched + changed;
    if denominator == 0 || changed as f64 / denominator as f64 > REFLOW_MAX_DISTANCE {
        return None;
    }

    Some((
        side_emphasis(minus_lines, &minus_tokens, &minus_changed),
        side_emphasis(plus_lines, &plus_tokens, &plus_changed),
    ))
}

pub(super) fn changed_width(lines: &[&str], emphasis: &[LineEmphasis]) -> usize {
    lines
        .iter()
        .zip(emphasis)
        .map(|(line, emphasis)| match emphasis {
            LineEmphasis::Plain => content_width(line),
            LineEmphasis::Paired(sections) => sections
                .iter()
                .filter(|s| s.kind == EmphKind::Emph)
                .map(|s| content_width(&s.text))
                .sum(),
        })
        .sum()
}

fn content_width(text: &str) -> usize {
    text.split_whitespace().map(UnicodeWidthStr::width).sum()
}

fn content_tokens<'a>(lines: &[&'a str]) -> Vec<Token<'a>> {
    let mut tokens = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let mut start = 0usize;
        for token in tokenize(text) {
            if !token.trim().is_empty() {
                tokens.push(Token {
                    line,
                    start,
                    text: token,
                });
            }
            start += token.len();
        }
    }
    tokens
}

fn side_emphasis(lines: &[&str], tokens: &[Token<'_>], changed: &[bool]) -> Vec<LineEmphasis> {
    let mut emphasis = Vec::with_capacity(lines.len());
    let mut next = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let first = next;
        while next < tokens.len() && tokens[next].line == index {
            next += 1;
        }
        if first == next {
            emphasis.push(LineEmphasis::Plain);
            continue;
        }

        let mut sections: Vec<EmphSection> = Vec::new();
        let mut offset = 0usize;
        for k in first..next {
            let token = &tokens[k];
            if token.start > offset {
                let gap_changed = k > first && changed[k - 1] && changed[k];
                push_section(&mut sections, kind(gap_changed), &line[offset..token.start]);
            }
            let end = token.start + token.text.len();
            push_section(&mut sections, kind(changed[k]), &line[token.start..end]);
            offset = end;
        }
        if offset < line.len() {
            push_section(&mut sections, EmphKind::NonEmph, &line[offset..]);
        }
        emphasis.push(LineEmphasis::Paired(sections));
    }
    emphasis
}

fn kind(changed: bool) -> EmphKind {
    if changed {
        EmphKind::Emph
    } else {
        EmphKind::NonEmph
    }
}

fn push_section(sections: &mut Vec<EmphSection>, kind: EmphKind, text: &str) {
    if let Some(last) = sections.last_mut()
        && last.kind == kind
    {
        last.text.push_str(text);
        return;
    }
    sections.push(EmphSection {
        kind,
        text: text.to_string(),
    });
}
