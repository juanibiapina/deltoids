//! Plain-text layout helpers shared by the TUI panes: display width
//! (tabs count as four columns) and word wrapping that falls back to
//! character splitting for words wider than the pane.

use unicode_width::UnicodeWidthChar;

pub(in crate::cli::browse) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if ch == '\t' {
                4
            } else {
                ch.width().unwrap_or(0)
            }
        })
        .sum()
}

fn split_word_to_width(word: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0usize;

    for ch in word.chars() {
        let ch_width = if ch == '\t' {
            4
        } else {
            ch.width().unwrap_or(0)
        };
        if chunk_width + ch_width > max_width && !chunk.is_empty() {
            lines.push(chunk);
            chunk = String::new();
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += ch_width;
    }

    if !chunk.is_empty() {
        lines.push(chunk);
    }

    lines
}

pub(in crate::cli::browse) fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = display_width(word);

        if word_width > max_width {
            // Word too long for a single line: flush current, then split by character.
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
                current_width = 0;
            }
            let mut chunks = split_word_to_width(word, max_width).into_iter();
            if let Some(last) = chunks.next_back() {
                lines.extend(chunks);
                current_width = display_width(&last);
                current = last;
            }
            continue;
        }

        if current.is_empty() {
            current = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= max_width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_fits_on_one_line() {
        assert_eq!(wrap_text("short", 80), vec!["short"]);
    }
    #[test]
    fn wrap_text_wraps_at_word_boundary() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }
    #[test]
    fn wrap_text_splits_long_word_by_character() {
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
    #[test]
    fn wrap_text_empty_string() {
        assert_eq!(wrap_text("", 80), vec![""]);
    }
    #[test]
    fn wrap_text_exact_fit() {
        assert_eq!(wrap_text("abcd", 4), vec!["abcd"]);
    }
    #[test]
    fn wrap_text_single_word_longer_than_width() {
        assert_eq!(wrap_text("abcdef", 4), vec!["abcd", "ef"]);
    }
    #[test]
    fn split_word_to_width_splits_by_display_width() {
        assert_eq!(
            split_word_to_width("abcdefghij", 4),
            vec!["abcd", "efgh", "ij"]
        );
    }
    #[test]
    fn wrap_text_mixed_short_and_long_words() {
        assert_eq!(
            wrap_text("hi abcdefgh there", 6),
            vec!["hi", "abcdef", "gh", "there"]
        );
    }
}
