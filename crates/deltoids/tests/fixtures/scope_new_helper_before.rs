fn flush_styled() {
    // existing helper
}

fn truncate_ranges(base_style: Style) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut width = 0;

    for ch in "example".chars() {
        // Old inline logic that will be replaced
        let ch_width = if ch == '\t' {
            4
        } else {
            ch.width().unwrap_or(0)
        };
        if ch_width == 0 {
            continue;
        }
        width += ch_width;
    }

    spans
}

fn other_function() {
    // unrelated function
}
