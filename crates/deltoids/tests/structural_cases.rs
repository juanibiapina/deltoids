//! Integration test entry point for the structural-diff reference suite.
//!
//! Mirrors the [`diff_cases`] suite but for [`deltoids::StructuralDiff`].
//! Each case directory under `tests/structural_cases/cases` describes
//! one scenario with `2-original.<EXT>` / `3-updated.<EXT>` plus an
//! `4-expected.structural` recording the structural-diff output in the
//! format produced by [`deltoids::structural::format_summary`].
//!
//! See `tests/structural_cases/README.md` for the format.

#[path = "structural_cases/harness.rs"]
mod harness;

use harness::{cases_root, discover_cases, report_failures, run_case, update_mode};

#[test]
fn all_structural_cases_match_expected_output() {
    let root = cases_root();
    let cases = discover_cases(&root);
    assert!(
        !cases.is_empty(),
        "no structural cases found under {}",
        root.display()
    );

    let update = update_mode();
    let mut failures = Vec::new();
    for case in &cases {
        if let Err(failure) = run_case(case, update) {
            failures.push(failure);
        }
    }

    if update {
        eprintln!(
            "Updated 4-expected.structural for {} case(s) under {}.",
            cases.len(),
            root.display()
        );
        return;
    }

    if !failures.is_empty() {
        panic!("{}", report_failures(&failures));
    }
}
