use deltoids::{Diff, LineKind};

#[test]
fn committed_scope_diff_has_no_context_only_default_context_range_hunk() {
    let original = include_str!("fixtures/scope_commit_before.rs");
    let updated = include_str!("fixtures/scope_commit_after.rs");

    let diff = Diff::compute(original, updated, "test.rs");

    let has_context_only_default_context_range_hunk = diff.hunks().iter().any(|hunk| {
        hunk.ancestors
            .iter()
            .any(|a| a.name == "default_context_range")
            && hunk.lines.iter().all(|line| line.kind == LineKind::Context)
    });

    assert!(
        !has_context_only_default_context_range_hunk,
        "should not emit a context-only hunk for default_context_range"
    );
}
