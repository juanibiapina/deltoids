//! Integration test entry point for the diff-case reference suite.
//!
//! Discovers every case directory under `tests/diff_cases/cases`, runs the
//! diff engine over its `before`/`after` files, and compares the result to
//! the case's `expected.diff`. Every case, and a sweep of seeded random
//! edits (each also expanded up to twice) over the case inputs, must also
//! satisfy the exact-cover invariant in `diff_cases/exact_cover.rs`.

#[path = "diff_cases/exact_cover.rs"]
mod exact_cover;
#[path = "diff_cases/harness.rs"]
mod harness;

use deltoids::Diff;
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

#[test]
fn every_case_shows_each_changed_line_exactly_once() {
    let mut failures = Vec::new();
    for case in discover_cases(&cases_root()) {
        let (original, updated) = (case.original(), case.updated());
        let diff = Diff::compute(&original, &updated, &case.diff_path());
        for error in exact_cover::violations(&original, &updated, diff.hunks()) {
            failures.push(format!("{}: {error}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const EDITS_PER_CASE: u64 = 12;

#[test]
fn random_edits_show_each_changed_line_exactly_once() {
    let mut failures = Vec::new();
    for case in discover_cases(&cases_root()) {
        let original = case.original();
        let lines: Vec<&str> = original.lines().collect();
        for seed in 1..=EDITS_PER_CASE {
            let updated = mutate(&lines, seed);
            if let Some(error) = expansion_violation(&original, &updated, &case.diff_path(), seed) {
                failures.push(format!(
                    "{} seed {seed} {error}\n--- updated ---\n{updated}",
                    case.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} random edit(s) broke exact cover:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The first exact-cover break in the diff of `original` and `updated`,
/// or in that diff after each of up to two seeded hunk expansions.
fn expansion_violation(original: &str, updated: &str, path: &str, seed: u64) -> Option<String> {
    let mut diff = Diff::compute(original, updated, path);
    for step in 0..=2 {
        if let Some(error) = exact_cover::violations(original, updated, diff.hunks()).first() {
            return Some(format!("after {step} expansion(s): {error}"));
        }
        let count = diff.hunks().len().max(1);
        diff = diff.expand((seed as usize + step) % count)?;
    }
    None
}

/// Apply one to four seeded line edits: insert, delete a run, change,
/// copy a block elsewhere, move a block, or insert a blank line.
fn mutate(lines: &[&str], seed: u64) -> String {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    for _ in 0..1 + rng.below(4) {
        let len = out.len();
        match rng.below(6) {
            0 => {
                let at = rng.below(len + 1);
                out.insert(at, format!("    let added_{} = 1;", rng.below(1000)));
            }
            1 if len > 0 => {
                let at = rng.below(len);
                let run = 1 + rng.below(8.min(len - at));
                out.drain(at..at + run);
            }
            2 if len > 0 => {
                let at = rng.below(len);
                out[at].push_str(" // changed");
            }
            3 if len > 0 => {
                let at = rng.below(len);
                let run = 1 + rng.below(15.min(len - at));
                let block = out[at..at + run].to_vec();
                let to = rng.below(len + 1);
                out.splice(to..to, block);
            }
            4 if len > 1 => {
                let at = rng.below(len);
                let run = 1 + rng.below(10.min(len - at));
                let block: Vec<String> = out.drain(at..at + run).collect();
                let to = rng.below(out.len() + 1);
                out.splice(to..to, block);
            }
            _ => {
                let at = rng.below(len + 1);
                out.insert(at, String::new());
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if bound == 0 {
            0
        } else {
            (self.0 % bound as u64) as usize
        }
    }
}
