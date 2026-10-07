struct Section<'a> {
    path: String,
    hunks: &'a [Hunk],
}

const X: u8 = 1;
