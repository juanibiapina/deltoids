//! Exact-cover invariant for `Diff::compute` hunks.
//!
//! Every removed and added line of the line-level diff appears in exactly
//! one hunk, hunks are ordered and disjoint on both sides, every hunk line
//! matches the source at the hunk's stated start, and no hunk is context
//! only. The reference line diff is computed here with the same algorithm
//! the engine uses (histogram plus line postprocessing), independently of
//! the engine's hunk construction.

use std::ops::Range;

use deltoids::{Hunk, LineKind};
use gix_imara_diff::{Algorithm, Diff, InternedInput};

/// Describe every way `hunks` break exact cover. Empty when they hold.
pub fn violations(original: &str, updated: &str, hunks: &[Hunk]) -> Vec<String> {
    let old: Vec<&str> = original.lines().collect();
    let new: Vec<&str> = updated.lines().collect();
    let mut errors = Vec::new();
    let mut removed_seen = vec![0usize; old.len()];
    let mut added_seen = vec![0usize; new.len()];
    let mut old_floor = 0;
    let mut new_floor = 0;

    for (index, hunk) in hunks.iter().enumerate() {
        if hunk.lines.iter().all(|line| line.kind == LineKind::Context) {
            errors.push(format!("hunk {index} has no changed line"));
        }
        let (Some(mut old_at), Some(mut new_at)) =
            (hunk.old_start.checked_sub(1), hunk.new_start.checked_sub(1))
        else {
            errors.push(format!("hunk {index} has a zero start"));
            continue;
        };
        if old_at < old_floor || new_at < new_floor {
            errors.push(format!(
                "hunk {index} starts at -{} +{} inside the previous hunk",
                hunk.old_start, hunk.new_start
            ));
        }
        for line in &hunk.lines {
            let content = line.content.as_str();
            let (old_ok, new_ok) = match line.kind {
                LineKind::Context => (
                    old.get(old_at) == Some(&content),
                    new.get(new_at) == Some(&content),
                ),
                LineKind::Removed => (old.get(old_at) == Some(&content), true),
                LineKind::Added => (true, new.get(new_at) == Some(&content)),
            };
            if !old_ok || !new_ok {
                errors.push(format!(
                    "hunk {index} line {content:?} does not match the source at old {} new {}",
                    old_at + 1,
                    new_at + 1
                ));
                break;
            }
            match line.kind {
                LineKind::Context => {
                    old_at += 1;
                    new_at += 1;
                }
                LineKind::Removed => {
                    removed_seen[old_at] += 1;
                    old_at += 1;
                }
                LineKind::Added => {
                    added_seen[new_at] += 1;
                    new_at += 1;
                }
            }
        }
        old_floor = old_at;
        new_floor = new_at;
    }

    let input = InternedInput::new(original, updated);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    for change in diff.hunks() {
        let removed = change.before.start as usize..change.before.end as usize;
        let added = change.after.start as usize..change.after.end as usize;
        expect_once(&mut errors, "removed old", removed, &mut removed_seen);
        expect_once(&mut errors, "added new", added, &mut added_seen);
    }
    for (line, _) in removed_seen.iter().enumerate().filter(|(_, n)| **n > 0) {
        errors.push(format!(
            "old line {} shown as removed but unchanged",
            line + 1
        ));
    }
    for (line, _) in added_seen.iter().enumerate().filter(|(_, n)| **n > 0) {
        errors.push(format!(
            "new line {} shown as added but unchanged",
            line + 1
        ));
    }
    errors
}

/// Check that each line in `lines` was shown once, then clear its count so
/// any count left afterwards marks an unchanged line shown as changed.
fn expect_once(errors: &mut Vec<String>, what: &str, lines: Range<usize>, seen: &mut [usize]) {
    for (offset, count) in seen[lines.clone()].iter_mut().enumerate() {
        let line = lines.start + offset + 1;
        match *count {
            1 => {}
            0 => errors.push(format!("{what} line {line} is missing")),
            n => errors.push(format!("{what} line {line} appears {n} times")),
        }
        *count = 0;
    }
}
