use deltoids::Diff;

#[test]
fn build_hunks_with_scope_hunk_keeps_its_function_ancestor() {
    let original = include_str!("fixtures/scope_before_comment_anchor.rs");
    let updated = include_str!("fixtures/scope_after_comment_anchor.rs");

    let diff = Diff::compute(original, updated, "test.rs");
    let hunk = diff
        .hunks()
        .iter()
        .find(|hunk| {
            hunk.lines
                .iter()
                .any(|line| line.content.contains("fn build_hunks_with_scope("))
        })
        .expect("missing build_hunks_with_scope hunk");

    assert_eq!(
        hunk.ancestors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        vec!["build_hunks_with_scope"]
    );
}
