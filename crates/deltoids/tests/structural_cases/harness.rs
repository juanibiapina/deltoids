//! Harness for the `structural_cases` integration test.
//!
//! Mirrors `diff_cases/harness.rs`. A "structural case" is a
//! self-contained directory under `cases/` capturing one scenario for
//! [`deltoids::StructuralDiff::compute`]:
//!
//! ```text
//! cases/<NNN-slug>/
//!   1-case.md                Markdown description.
//!   2-original.<EXT>         Pre-edit source.
//!   3-updated.<EXT>          Post-edit source (matching extension).
//!   4-expected.structural    Recorded output of `format_summary`.
//! ```
//!
//! The expected file is exactly the string produced by
//! [`deltoids::structural::format_summary`], including the title line
//! and the bullet-prefixed change list. An empty diff (no structural
//! changes) is represented by an empty file.
//!
//! Set `DELTOIDS_UPDATE_CASES=1` to overwrite each `4-expected.structural`
//! with the current actual output. Use this when adding a new case;
//! manually inspect the generated file and commit.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use deltoids::StructuralDiff;
use deltoids::structural::format_summary;

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

    /// File-name passed to `StructuralDiff::compute`. Strips the
    /// `2-` prefix so language detection sees `original.<EXT>`, not
    /// `2-original.<EXT>`.
    pub fn diff_path(&self) -> String {
        let name = self
            .original_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("original");
        name.strip_prefix("2-").unwrap_or(name).to_string()
    }
}

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
        let expected = path.join("4-expected.structural");
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

pub struct CaseFailure {
    pub name: String,
    pub dir: PathBuf,
    pub expected: String,
    pub actual: String,
}

pub fn run_case(case: &Case, update: bool) -> Result<(), CaseFailure> {
    let original = case.original();
    let updated = case.updated();
    let path = case.diff_path();
    let structural = StructuralDiff::compute(&original, &updated, &path);
    let actual = format_summary(&structural);

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

pub fn cases_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("structural_cases")
        .join("cases")
}

pub fn update_mode() -> bool {
    std::env::var("DELTOIDS_UPDATE_CASES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn report_failures(failures: &[CaseFailure]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} structural case(s) failed.\n\n\
         Re-run with `DELTOIDS_UPDATE_CASES=1 cargo test -p deltoids --test structural_cases`\n\
         to refresh `4-expected.structural` files, then review the changes.\n\n",
        failures.len()
    ));
    for failure in failures {
        out.push_str(&format!("--- case: {} ---\n", failure.name));
        out.push_str(&format!("  dir: {}\n", failure.dir.display()));
        out.push_str("  expected:\n");
        for line in failure.expected.lines() {
            out.push_str(&format!("    {line}\n"));
        }
        out.push_str("  actual:\n");
        for line in failure.actual.lines() {
            out.push_str(&format!("    {line}\n"));
        }
        out.push('\n');
    }
    out
}
