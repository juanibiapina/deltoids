//! Test that non-diff input is passed through unchanged.

use deltoids::parse::GitDiff;

#[test]
fn parse_non_diff_input_should_preserve_content() {
    // git log --oneline style output
    let input = "abc1234 First commit
def5678 Second commit
ghi9abc Third commit";

    let parsed = GitDiff::parse(input);

    // Bug: non-diff input results in empty files, silently dropping content
    // Fix: add trailing_preamble field to capture orphaned preamble
    assert!(
        !parsed.files.is_empty() || parsed.trailing_preamble.is_some(),
        "Non-diff input should be captured in trailing_preamble, not silently dropped"
    );
}

#[test]
fn parse_regular_git_log_should_preserve_content() {
    // git log (default format) style output
    let input = "commit abc1234567890
Author: Test User <test@example.com>
Date:   Mon Jan 1 12:00:00 2024 +0000

    Add feature X
";

    let parsed = GitDiff::parse(input);

    // Bug: non-diff git log output results in empty files
    assert!(
        !parsed.files.is_empty() || parsed.trailing_preamble.is_some(),
        "Non-diff git log output should be captured in trailing_preamble"
    );
}

#[test]
fn parse_diff_with_trailing_log_preserves_both() {
    // git log -p can have commit info after the last diff
    let input = r#"commit abc1234
Author: User <user@example.com>

    First commit

diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
+new

commit def5678
Author: User <user@example.com>

    Second commit (no diff, maybe empty commit or merge)
"#;

    let parsed = GitDiff::parse(input);

    assert_eq!(parsed.files.len(), 1, "Should have one file diff");
    // The second commit info should be captured as trailing_preamble
    assert!(
        parsed.trailing_preamble.is_some(),
        "Trailing commit metadata should be captured"
    );
    let trailing = parsed.trailing_preamble.as_ref().unwrap();
    assert!(
        trailing.iter().any(|line| line.contains("def5678")),
        "Trailing preamble should contain second commit hash"
    );
}
