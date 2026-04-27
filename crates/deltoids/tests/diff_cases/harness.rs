//! Harness for the `diff_cases` integration test.
//!
//! A "diff case" is a self-contained directory under `cases/` that captures
//! one diff scenario as a runnable spec. Each case is both a test and a
//! product reference document.
//!
//! ## Case directory layout
//!
//! ```text
//! cases/<NNN-slug>/
//!   1-case.md          Markdown description (title, why this case
//!                      exists, behaviours pinned, manual review notes).
//!   2-original.<EXT>   File content before the edit. The extension picks
//!                      the tree-sitter language used by the diff engine.
//!   3-updated.<EXT>    File content after the edit. Must use the same EXT.
//!   4-expected.diff    Expected output of `Diff::compute(original,
//!                      updated, "original.<EXT>")` rendered in the case
//!                      format below.
//! ```
//!
//! Files are numbered so a directory listing reads in the natural order
//! of the test: description, then the two inputs (matching the
//! `original` / `updated` parameters of `Diff::compute`), then the
//! expected output.
//!
//! A case directory whose name starts with `_` is skipped (useful for
//! drafts).
//!
//! ## Case format
//!
//! `expected.diff` looks like a unified diff, with one extension: the line
//! after `@@` carries the hunk's ancestor breadcrumb chain, written as
//! whitespace-separated `[KIND name]` chunks (outermost first). When a hunk
//! has no ancestors the breadcrumb section is empty.
//!
//! ```text
//! @@ -1,5 +1,5 @@ [impl_item Foo] [function_item compute]
//!  fn compute(&self) -> i32 {
//! -    x + 1
//! +    x + 2
//!  }
//! ```
//!
//! Hunks are separated by their `@@` headers, in the order the diff engine
//! emits them. A diff with no hunks (identical files) is the empty string.
//!
//! ## Running cases
//!
//! `cargo test -p deltoids --test diff_cases` runs every case. A failing
//! case prints a unified diff between the expected and the actual output,
//! plus the path to the case directory.
//!
//! Set `DELTOIDS_UPDATE_CASES=1` to overwrite each `4-expected.diff`
//! with the current actual output. Use this when adding a new case
//! (write `1-case.md`, `2-original.<EXT>`, `3-updated.<EXT>`, run with
//! the env var, manually inspect the generated `4-expected.diff`,
//! commit).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use deltoids::{Diff, Hunk, LineKind};

// ---------------------------------------------------------------------------
// Case discovery and loading
// ---------------------------------------------------------------------------

/// One discovered diff case on disk.
pub struct Case {
    pub name: String,
    pub dir: PathBuf,
    pub original_path: PathBuf,
    pub updated_path: PathBuf,
    pub expected_path: PathBuf,
}

impl Case {
    pub fn original(&self) -> String {
        fs::read_to_string(&self.original_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", self.original_path.display()))
    }

    pub fn updated(&self) -> String {
        fs::read_to_string(&self.updated_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", self.updated_path.display()))
    }

    pub fn expected(&self) -> String {
        fs::read_to_string(&self.expected_path).unwrap_or_default()
    }

    /// File-name passed to `Diff::compute`. The diff engine uses this only
    /// to pick the tree-sitter language; the actual file is not read again.
    /// We strip the numeric `2-` prefix so that the language detection sees
    /// e.g. `original.rs`, not `2-original.rs`.
    pub fn diff_path(&self) -> String {
        let name = self
            .original_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("original");
        name.strip_prefix("2-").unwrap_or(name).to_string()
    }
}

/// Find every case directory under `cases_root`. A case directory must
/// contain `2-original.<EXT>` and `3-updated.<EXT>` with matching
/// extensions, plus a writable slot for `4-expected.diff`.
pub fn discover_cases(cases_root: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    let entries = fs::read_dir(cases_root)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", cases_root.display()));

    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || name.starts_with('_') || name.starts_with('.') {
            continue;
        }

        let (original, updated) = locate_original_updated(&path);
        let expected = path.join("4-expected.diff");
        cases.push(Case {
            name,
            dir: path,
            original_path: original,
            updated_path: updated,
            expected_path: expected,
        });
    }

    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

fn locate_original_updated(case_dir: &Path) -> (PathBuf, PathBuf) {
    let mut original: Option<PathBuf> = None;
    let mut updated: Option<PathBuf> = None;
    let entries =
        fs::read_dir(case_dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", case_dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let stem = path.file_stem().and_then(OsStr::to_str).unwrap_or("");
        match stem {
            "2-original" => original = Some(path),
            "3-updated" => updated = Some(path),
            _ => {}
        }
    }
    let original = original.unwrap_or_else(|| {
        panic!(
            "case {} is missing a `2-original.<EXT>` file",
            case_dir.display()
        )
    });
    let updated = updated.unwrap_or_else(|| {
        panic!(
            "case {} is missing a `3-updated.<EXT>` file",
            case_dir.display()
        )
    });
    if original.extension() != updated.extension() {
        panic!(
            "case {} has mismatched extensions for original/updated",
            case_dir.display()
        );
    }
    (original, updated)
}

// ---------------------------------------------------------------------------
// Case format: serialising hunks back to text
// ---------------------------------------------------------------------------

/// Render a sequence of hunks in the case format. Returns "" when `hunks`
/// is empty.
pub fn format_hunks(hunks: &[Hunk]) -> String {
    let mut out = String::new();
    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_hunk_into(hunk, &mut out);
    }
    out
}

fn format_hunk_into(hunk: &Hunk, out: &mut String) {
    let (old_count, new_count) = count_old_new(hunk);
    let header_old = format_range(hunk.old_start, old_count);
    let header_new = format_range(hunk.new_start, new_count);

    let breadcrumb = format_ancestors(hunk);
    if breadcrumb.is_empty() {
        out.push_str(&format!("@@ -{header_old} +{header_new} @@\n"));
    } else {
        out.push_str(&format!("@@ -{header_old} +{header_new} @@ {breadcrumb}\n"));
    }

    for line in &hunk.lines {
        let prefix = match line.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        };
        out.push(prefix);
        out.push_str(&line.content);
        out.push('\n');
    }
}

fn count_old_new(hunk: &Hunk) -> (usize, usize) {
    let mut old = 0usize;
    let mut new = 0usize;
    for line in &hunk.lines {
        match line.kind {
            LineKind::Context => {
                old += 1;
                new += 1;
            }
            LineKind::Removed => old += 1,
            LineKind::Added => new += 1,
        }
    }
    (old, new)
}

fn format_range(start: usize, count: usize) -> String {
    if count == 1 {
        format!("{start}")
    } else {
        format!("{start},{count}")
    }
}

fn format_ancestors(hunk: &Hunk) -> String {
    hunk.ancestors
        .iter()
        .map(|a| format!("[{} {}]", a.kind, a.name))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Case execution
// ---------------------------------------------------------------------------

pub struct CaseFailure {
    pub name: String,
    pub dir: PathBuf,
    pub expected: String,
    pub actual: String,
}

/// Run a single case: compute the diff, format it, compare to expected.
/// In update mode (`DELTOIDS_UPDATE_CASES=1`), overwrite `expected.diff`
/// instead of comparing.
pub fn run_case(case: &Case, update: bool) -> Result<(), CaseFailure> {
    let original = case.original();
    let updated = case.updated();
    let diff_path = case.diff_path();
    let diff = Diff::compute(&original, &updated, &diff_path);
    let actual = format_hunks(diff.hunks());

    if update {
        fs::write(&case.expected_path, &actual)
            .unwrap_or_else(|e| panic!("write {}: {e}", case.expected_path.display()));
        return Ok(());
    }

    let expected = case.expected();
    if expected == actual {
        Ok(())
    } else {
        Err(CaseFailure {
            name: case.name.clone(),
            dir: case.dir.clone(),
            expected,
            actual,
        })
    }
}

/// Path to the directory holding all on-disk cases.
pub fn cases_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("diff_cases")
        .join("cases")
}

/// True when the user requested update mode.
pub fn update_mode() -> bool {
    std::env::var("DELTOIDS_UPDATE_CASES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Pretty-print a list of failures. Used by the integration test entry
/// point.
pub fn report_failures(failures: &[CaseFailure]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} diff case(s) failed.\n\n\
         Re-run with `DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test diff_cases`\n\
         to refresh `4-expected.diff` files, then review the changes.\n\n",
        failures.len()
    ));
    for failure in failures {
        out.push_str(&format!("--- case: {} ---\n", failure.name));
        out.push_str(&format!("  dir: {}\n", failure.dir.display()));
        out.push_str("  diff (expected vs actual):\n");
        out.push_str(&unified_diff(&failure.expected, &failure.actual));
        out.push('\n');
    }
    out
}

fn unified_diff(expected: &str, actual: &str) -> String {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(expected, actual);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            ChangeTag::Equal => "    ",
            ChangeTag::Delete => "  - ",
            ChangeTag::Insert => "  + ",
        };
        out.push_str(prefix);
        out.push_str(change.value());
        if !change.value().ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Unit tests for the harness itself
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use deltoids::{DiffLine, ScopeNode};

    fn line(kind: LineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
        }
    }

    #[test]
    fn format_hunks_empty_returns_empty_string() {
        assert_eq!(format_hunks(&[]), "");
    }

    #[test]
    fn format_hunks_single_replacement_no_ancestors() {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            ancestors: Vec::new(),
            lines: vec![
                line(LineKind::Context, "alpha"),
                line(LineKind::Removed, "old"),
                line(LineKind::Added, "new"),
                line(LineKind::Context, "omega"),
            ],
        };
        let s = format_hunks(&[hunk]);
        assert_eq!(s, "@@ -1,3 +1,3 @@\n alpha\n-old\n+new\n omega\n");
    }

    #[test]
    fn format_hunks_single_line_uses_compact_range() {
        let hunk = Hunk {
            old_start: 7,
            new_start: 7,
            ancestors: Vec::new(),
            lines: vec![line(LineKind::Removed, "x"), line(LineKind::Added, "y")],
        };
        let s = format_hunks(&[hunk]);
        assert_eq!(s, "@@ -7 +7 @@\n-x\n+y\n");
    }

    #[test]
    fn format_hunks_emits_ancestor_chain() {
        let hunk = Hunk {
            old_start: 5,
            new_start: 5,
            ancestors: vec![
                ScopeNode {
                    kind: "impl_item".to_string(),
                    name: "Foo".to_string(),
                    start_line: 1,
                    end_line: 20,
                    text: "impl Foo {".to_string(),
                },
                ScopeNode {
                    kind: "function_item".to_string(),
                    name: "compute".to_string(),
                    start_line: 3,
                    end_line: 10,
                    text: "    fn compute() {".to_string(),
                },
            ],
            lines: vec![line(LineKind::Removed, "a"), line(LineKind::Added, "b")],
        };
        let s = format_hunks(&[hunk]);
        assert_eq!(
            s,
            "@@ -5 +5 @@ [impl_item Foo] [function_item compute]\n-a\n+b\n"
        );
    }

    #[test]
    fn format_hunks_multiple_hunks_separated_by_blank_line() {
        let hunk_a = Hunk {
            old_start: 1,
            new_start: 1,
            ancestors: Vec::new(),
            lines: vec![line(LineKind::Added, "first")],
        };
        let hunk_b = Hunk {
            old_start: 10,
            new_start: 11,
            ancestors: Vec::new(),
            lines: vec![line(LineKind::Removed, "second")],
        };
        let s = format_hunks(&[hunk_a, hunk_b]);
        assert_eq!(s, "@@ -1,0 +1 @@\n+first\n\n@@ -10 +11,0 @@\n-second\n");
    }
}
