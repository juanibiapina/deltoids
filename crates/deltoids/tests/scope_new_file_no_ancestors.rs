use deltoids::Diff;

/// Bug: When computing a diff for a new file (empty original), the scope
/// detection still runs and produces nested ancestor scope boxes. For a new
/// file where everything is added, no scope box should be shown since the
/// entire file is the "context".
#[test]
fn new_file_should_have_no_ancestors() {
    let original = "";
    let updated = r#"jobs:
  build:
    steps:
      - name: checkout
        with:
          key: value
"#;

    let diff = Diff::compute(original, updated, "test.yml");

    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1, "should have exactly one hunk for new file");

    let hunk = &hunks[0];
    assert!(
        hunk.ancestors.is_empty(),
        "new file should have no ancestors, but got: {:?}",
        hunk.ancestors
    );
}
