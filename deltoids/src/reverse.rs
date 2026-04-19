//! Reconstruct "before" content by reverse-applying a diff to "after" content.

use crate::parse::{FileDiff, RawLineKind};

/// Reconstruct the original "before" content from the current "after" content
/// and the diff that was applied.
///
/// This works by:
/// 1. Starting with the "after" content as lines
/// 2. For each hunk (in reverse order to avoid index shifts):
///    - Remove added lines (+)
///    - Insert removed lines (-) back
pub fn reconstruct_before(after_content: &str, file_diff: &FileDiff) -> String {
    let mut lines: Vec<String> = after_content.lines().map(String::from).collect();

    // Process hunks in reverse order to avoid index shifting issues
    for hunk in file_diff.hunks.iter().rev() {
        // We need to process the hunk and rebuild the affected region
        let mut new_region = Vec::new();

        for raw_line in &hunk.lines {
            match raw_line.kind {
                RawLineKind::Context => {
                    // Context line exists in both old and new
                    new_region.push(raw_line.content.clone());
                }
                RawLineKind::Added => {
                    // Added line: exists in new but not old, so skip it for "before"
                }
                RawLineKind::Removed => {
                    // Removed line: exists in old but not new, so add it back
                    new_region.push(raw_line.content.clone());
                }
            }
        }

        // Count how many lines in "after" this hunk covers
        let mut after_line_count = 0;
        for raw_line in &hunk.lines {
            match raw_line.kind {
                RawLineKind::Context | RawLineKind::Added => after_line_count += 1,
                RawLineKind::Removed => {}
            }
        }

        // Replace the affected region
        let start = hunk.new_start.saturating_sub(1);
        let end = (start + after_line_count).min(lines.len());

        // Remove the "after" lines and insert the "before" lines
        lines.splice(start..end, new_region);
    }

    // Join with newlines, preserving trailing newline if original had one
    let result = lines.join("\n");
    if after_content.ends_with('\n') && !result.is_empty() {
        format!("{result}\n")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ParsedDiff;

    fn parse_and_get_file(diff: &str) -> FileDiff {
        let parsed = ParsedDiff::parse(diff);
        parsed.files.into_iter().next().unwrap()
    }

    #[test]
    fn reconstruct_simple_addition() {
        let diff = r#"--- a/test.txt
+++ b/test.txt
@@ -1,2 +1,3 @@
 line1
+added
 line2
"#;
        let after = "line1\nadded\nline2\n";
        let file_diff = parse_and_get_file(diff);
        let before = reconstruct_before(after, &file_diff);
        assert_eq!(before, "line1\nline2\n");
    }

    #[test]
    fn reconstruct_simple_removal() {
        let diff = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,2 @@
 line1
-removed
 line2
"#;
        let after = "line1\nline2\n";
        let file_diff = parse_and_get_file(diff);
        let before = reconstruct_before(after, &file_diff);
        assert_eq!(before, "line1\nremoved\nline2\n");
    }

    #[test]
    fn reconstruct_modification() {
        let diff = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line1
-old line
+new line
 line3
"#;
        let after = "line1\nnew line\nline3\n";
        let file_diff = parse_and_get_file(diff);
        let before = reconstruct_before(after, &file_diff);
        assert_eq!(before, "line1\nold line\nline3\n");
    }

    #[test]
    fn reconstruct_multiple_hunks() {
        let diff = r#"--- a/test.txt
+++ b/test.txt
@@ -1,2 +1,2 @@
-first old
+first new
 middle
@@ -5,2 +5,2 @@
 context
-last old
+last new
"#;
        let after = "first new\nmiddle\nline3\nline4\ncontext\nlast new\n";
        let file_diff = parse_and_get_file(diff);
        let before = reconstruct_before(after, &file_diff);
        assert_eq!(before, "first old\nmiddle\nline3\nline4\ncontext\nlast old\n");
    }
}
