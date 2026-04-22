use deltoids::{Diff, LineKind};

/// Bug: When a new helper function is added and an adjacent function is modified
/// to use it, the helper can appear twice:
/// 1. In its own hunk (as a newly added function)
/// 2. As context in the modified function's hunk
///
/// This tests the pattern from src/highlight.rs where visible_char was added
/// and truncate_highlighted_ranges was modified to use it.
#[test]
fn new_helper_does_not_appear_in_modified_sibling_hunk() {
    let original = include_str!("fixtures/scope_new_helper_before.rs");
    let updated = include_str!("fixtures/scope_new_helper_after.rs");

    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Find hunks that contain visible_char
    let hunks_with_visible_char: Vec<_> = hunks
        .iter()
        .filter(|hunk| {
            hunk.lines
                .iter()
                .any(|line| line.content.contains("fn visible_char"))
        })
        .collect();

    // The new helper should appear in exactly ONE hunk, not multiple
    assert_eq!(
        hunks_with_visible_char.len(),
        1,
        "visible_char should appear in exactly 1 hunk, but found {} hunks containing it:\n{}",
        hunks_with_visible_char.len(),
        hunks_with_visible_char
            .iter()
            .map(|h| format!(
                "  hunk with ancestors: {:?}",
                h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // The hunk with visible_char should have a new scope as an ancestor
    // (either the function visible_char or the struct VisibleChar)
    let visible_char_hunk = hunks_with_visible_char[0];
    let has_new_scope_ancestor = visible_char_hunk
        .ancestors
        .iter()
        .any(|a| a.name == "visible_char" || a.name == "VisibleChar");
    assert!(
        has_new_scope_ancestor,
        "the hunk containing visible_char should have a new scope ancestor, got: {:?}",
        visible_char_hunk
            .ancestors
            .iter()
            .map(|a| &a.name)
            .collect::<Vec<_>>()
    );

    // The truncate_ranges hunk should NOT contain visible_char as context
    let truncate_hunks: Vec<_> = hunks
        .iter()
        .filter(|hunk| hunk.ancestors.iter().any(|a| a.name == "truncate_ranges"))
        .collect();

    for hunk in &truncate_hunks {
        let has_visible_char_context = hunk
            .lines
            .iter()
            .any(|line| line.kind == LineKind::Context && line.content.contains("fn visible_char"));
        assert!(
            !has_visible_char_context,
            "truncate_ranges hunk should not include visible_char as context"
        );
    }
}

/// Additional check: the new struct (VisibleChar) should also not appear
/// duplicated across hunks.
#[test]
fn new_struct_does_not_appear_in_modified_sibling_hunk() {
    let original = include_str!("fixtures/scope_new_helper_before.rs");
    let updated = include_str!("fixtures/scope_new_helper_after.rs");

    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Find hunks containing the VisibleChar struct definition
    let hunks_with_struct: Vec<_> = hunks
        .iter()
        .filter(|hunk| {
            hunk.lines
                .iter()
                .any(|line| line.content.contains("struct VisibleChar"))
        })
        .collect();

    // The struct should appear in exactly ONE hunk
    assert_eq!(
        hunks_with_struct.len(),
        1,
        "VisibleChar struct should appear in exactly 1 hunk, but found {} hunks",
        hunks_with_struct.len()
    );
}
