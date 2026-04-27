//! Integration test entry point for the diff-case reference suite.
//!
//! Discovers every case directory under `tests/diff_cases/cases`, runs the
//! diff engine over its `before`/`after` files, and compares the result to
//! the case's `expected.diff`. See [`diff_cases::mod`] for the case format.

#[path = "diff_cases/harness.rs"]
mod harness;

use harness::{cases_root, discover_cases, report_failures, run_case, update_mode};

#[test]
fn all_diff_cases_match_expected_output() {
    let root = cases_root();
    let cases = discover_cases(&root);
    assert!(
        !cases.is_empty(),
        "no diff cases found under {}",
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
        // In update mode, never fail; report what was rewritten.
        eprintln!(
            "Updated expected.diff for {} case(s) under {}.",
            cases.len(),
            root.display()
        );
        return;
    }

    if !failures.is_empty() {
        panic!("{}", report_failures(&failures));
    }
}
