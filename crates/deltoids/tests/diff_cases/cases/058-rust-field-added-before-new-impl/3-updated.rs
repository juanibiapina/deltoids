struct Section<'a> {
    path: String,
    hunks: &'a [Hunk],
    numbering: Numbering,
}

impl Section<'_> {
    fn lines(&self) -> usize {
        self.hunks.len()
    }
}

const X: u8 = 1;
