fn flush_styled() {
    // existing helper
}

struct VisibleChar {
    text: String,
    width: usize,
}

fn visible_char(ch: char) -> Option<VisibleChar> {
    if ch == '\t' {
        return Some(VisibleChar {
            text: "    ".to_string(),
            width: 4,
        });
    }
    let width = ch.width().unwrap_or(0);
    if width == 0 {
        return None;
    }
    Some(VisibleChar {
        text: ch.to_string(),
        width,
    })
}

fn truncate_ranges(base_style: Style) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut width = 0;

    for ch in "example".chars() {
        // New logic using helper
        let Some(visible) = visible_char(ch) else {
            continue;
        };
        width += visible.width;
    }

    spans
}

fn other_function() {
    // unrelated function
}
