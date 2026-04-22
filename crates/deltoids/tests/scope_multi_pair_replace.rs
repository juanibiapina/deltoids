//! Test for multi-pair replace context bug.
//!
//! When a Replace operation spans multiple object property pairs (like `{ a: X, b: Y, c: Z }`),
//! deltoids should show all changes together in a single hunk, not fragment them across
//! multiple hunks with incomplete context.
//!
//! Bug: When replacing multiple adjacent `pair` nodes within a method, deltoids:
//! 1. Creates separate hunks for different pairs
//! 2. The first hunk shows only the first replacement (missing the rest)
//! 3. The second hunk shows only added lines (missing corresponding deletions)
//! 4. Some changes may be missing entirely

use deltoids::{Diff, LineKind};

/// Debug test that prints the actual hunks produced.
/// Run with: cargo test -p deltoids --test scope_multi_pair_replace -- debug_print --nocapture
#[test]
#[ignore] // Run manually to see output
fn debug_print_actual_hunks() {
    let before = read_fixture("scope_multi_pair_before.ts");
    let after = read_fixture("scope_multi_pair_after.ts");

    let diff = Diff::compute(&before, &after, "task.service.ts");
    let hunks = diff.hunks();

    println!("\n=== ACTUAL HUNKS ===");
    for (i, hunk) in hunks.iter().enumerate() {
        println!(
            "\n--- Hunk {} (old_start: {}, new_start: {}) ---",
            i, hunk.old_start, hunk.new_start
        );
        println!(
            "Ancestors: {:?}",
            hunk.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>()
        );
        for line in &hunk.lines {
            let prefix = match line.kind {
                LineKind::Added => "+",
                LineKind::Removed => "-",
                LineKind::Context => " ",
            };
            println!("{}{}", prefix, line.content);
        }
    }
    println!("\n=== END HUNKS ===");

    // Fail to ensure output is shown
    panic!("Debug test - see output above");
}

fn read_fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
}

/// This test documents the bug where a single Replace operation (4 deleted lines, 3 added)
/// is fragmented into multiple hunks with incomplete content.
///
/// The diff should show ALL deleted lines paired with ALL added lines,
/// but instead it fragments the display.
#[test]
fn multi_pair_replace_should_show_all_changes_in_single_hunk() {
    let before = read_fixture("scope_multi_pair_before.ts");
    let after = read_fixture("scope_multi_pair_after.ts");

    let diff = Diff::compute(&before, &after, "task.service.ts");
    let hunks = diff.hunks();

    // We expect the method body changes to be in a SINGLE hunk.
    // The import change is separate (that's fine).
    //
    // Currently buggy: deltoids creates multiple hunks for the method body,
    // fragmenting the Replace operation.

    // Count hunks that contain method body changes (TYPE_A, TYPE_B, TYPE_C lines)
    let method_body_hunks: Vec<_> = hunks
        .iter()
        .filter(|h| {
            h.lines
                .iter()
                .any(|line| line.content.contains("TaskType.TYPE"))
        })
        .collect();

    // BUG: This assertion documents the expected behavior.
    // A single Replace operation should produce a single hunk.
    assert_eq!(
        method_body_hunks.len(),
        1,
        "Expected 1 hunk for method body changes, but got {}. \
         A Replace operation spanning multiple pairs should not be fragmented.",
        method_body_hunks.len()
    );
}

/// Test that all deleted lines appear in the output.
///
/// When replacing 4 lines (TYPE_A, TYPE_B, and TYPE_C which spans 2 lines),
/// all deleted lines should appear somewhere in the diff hunks.
#[test]
fn multi_pair_replace_should_include_all_deleted_lines() {
    let before = read_fixture("scope_multi_pair_before.ts");
    let after = read_fixture("scope_multi_pair_after.ts");

    let diff = Diff::compute(&before, &after, "task.service.ts");
    let hunks = diff.hunks();

    // Collect all removed lines from all hunks
    let removed_lines: Vec<&str> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|line| line.kind == deltoids::LineKind::Removed)
        .map(|line| line.content.as_str())
        .collect();

    // We should see all 4 deleted lines
    let expected_deletions = [
        "OLD_A",    // TYPE_A line
        "OLD_B",    // TYPE_B line
        "TYPE_C]:", // TYPE_C line (split across 2 lines in old)
        "OLD_C",    // continuation of TYPE_C
    ];

    for expected in &expected_deletions {
        assert!(
            removed_lines.iter().any(|line| line.contains(expected)),
            "Missing deleted content '{}' in removed lines: {:?}",
            expected,
            removed_lines
        );
    }
}

/// Test that all added lines appear in the output.
///
/// When adding 3 lines (TYPE_A, TYPE_B, TYPE_C replacements),
/// all added lines should appear somewhere in the diff hunks.
#[test]
fn multi_pair_replace_should_include_all_added_lines() {
    let before = read_fixture("scope_multi_pair_before.ts");
    let after = read_fixture("scope_multi_pair_after.ts");

    let diff = Diff::compute(&before, &after, "task.service.ts");
    let hunks = diff.hunks();

    // Collect all added lines from all hunks
    let added_lines: Vec<&str> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|line| line.kind == deltoids::LineKind::Added)
        .map(|line| line.content.as_str())
        .collect();

    // We should see all 3 added lines
    let expected_additions = [
        "NEW.a.value", // TYPE_A line
        "NEW.b.value", // TYPE_B line
        "NEW.c.value", // TYPE_C line
    ];

    for expected in &expected_additions {
        assert!(
            added_lines.iter().any(|line| line.contains(expected)),
            "Missing added content '{}' in added lines: {:?}",
            expected,
            added_lines
        );
    }
}

/// Test that removed and added lines for the same replacement are in the same hunk.
///
/// A Replace operation should show removed lines immediately followed by added lines,
/// not split across different hunks.
#[test]
fn multi_pair_replace_removed_and_added_should_be_adjacent() {
    let before = read_fixture("scope_multi_pair_before.ts");
    let after = read_fixture("scope_multi_pair_after.ts");

    let diff = Diff::compute(&before, &after, "task.service.ts");
    let hunks = diff.hunks();

    // Find the hunk(s) that contain TYPE_B changes
    let type_b_hunks: Vec<_> = hunks
        .iter()
        .filter(|h| h.lines.iter().any(|line| line.content.contains("TYPE_B")))
        .collect();

    assert!(!type_b_hunks.is_empty(), "No hunk contains TYPE_B changes");

    // For each hunk with TYPE_B, check if it has both removed and added TYPE_B lines
    for hunk in &type_b_hunks {
        let has_removed_type_b = hunk.lines.iter().any(|line| {
            line.kind == deltoids::LineKind::Removed && line.content.contains("TYPE_B")
        });
        let has_added_type_b = hunk
            .lines
            .iter()
            .any(|line| line.kind == deltoids::LineKind::Added && line.content.contains("TYPE_B"));

        // BUG: A Replace hunk should have BOTH the removed and added versions.
        // Currently, one hunk might only have the removed line, and another only the added.
        assert!(
            has_removed_type_b && has_added_type_b,
            "Hunk should contain both removed and added TYPE_B lines. \
             Has removed: {}, has added: {}. \
             Removed and added lines for a Replace should be in the same hunk.",
            has_removed_type_b,
            has_added_type_b
        );
    }
}
