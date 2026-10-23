//! Integration test entry point for the diff-case reference suite.
//!
//! Discovers every case directory under `tests/diff_cases/cases`, runs the
//! diff engine over its `before`/`after` files, and compares the result to
//! the case's `expected.diff`. Every case, and a sweep of seeded random
//! edits (each also expanded up to twice, or revealing a hidden line)
//! over the case inputs, must also satisfy the exact-cover invariant in
//! `diff_cases/exact_cover.rs`.

#[path = "diff_cases/exact_cover.rs"]
mod exact_cover;
#[path = "diff_cases/harness.rs"]
mod harness;

use deltoids::{DefinitionChange, DefinitionDiff, Diff, Status};
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

#[test]
fn random_reveals_show_the_line_and_keep_exact_cover() {
    let mut failures = Vec::new();
    for case in discover_cases(&cases_root()) {
        let original = case.original();
        let lines: Vec<&str> = original.lines().collect();
        for seed in 1..=EDITS_PER_CASE {
            let updated = mutate(&lines, seed);
            if let Some(error) = reveal_violation(&original, &updated, &case.diff_path(), seed) {
                failures.push(format!(
                    "{} seed {seed} {error}\n--- updated ---\n{updated}",
                    case.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} random reveal(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn definition_changes_are_symmetric_and_explain_every_changed_line() {
    let mut failures = Vec::new();
    for case in discover_cases(&cases_root()) {
        let path = case.diff_path();
        let original = case.original();
        let lines: Vec<&str> = original.lines().collect();
        let mut pairs = vec![("case".to_string(), original.clone(), case.updated())];
        for seed in 1..=EDITS_PER_CASE {
            pairs.push((
                format!("seed {seed}"),
                original.clone(),
                mutate(&lines, seed),
            ));
        }
        for (label, before, after) in pairs {
            for error in definition_violations(&before, &after, &path) {
                failures.push(format!(
                    "{} {label}: {error}\n--- before ---\n{before}--- after ---\n{after}",
                    case.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} definition check(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn blank_lines_inside_definitions_are_formatting_only() {
    let mut failures = Vec::new();
    for case in discover_cases(&cases_root()) {
        let path = case.diff_path();
        // Whitespace means something in these.
        if [".py", ".yaml", ".yml", ".md"]
            .iter()
            .any(|ext| path.ends_with(ext))
        {
            continue;
        }
        for before in [case.original(), case.updated()] {
            let after = with_blank_lines(&before);
            if let Some(error) = non_formatting_changes(&before, &after, &path) {
                failures.push(format!("{}: {error}", case.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `text` with a blank line after every line that opens a block.
fn with_blank_lines(text: &str) -> String {
    text.lines()
        .map(|line| match line.trim_end().ends_with('{') {
            true => format!("{line}\n\n"),
            false => format!("{line}\n"),
        })
        .collect()
}

/// The changes from `before` to `after` that are not formatting, when
/// there are any.
fn non_formatting_changes(before: &str, after: &str, path: &str) -> Option<String> {
    let diff = DefinitionDiff::compute(before, after, path)?;
    if diff.has_errors {
        return None;
    }
    let formatting = format!(
        "{:?}",
        deltoids::Aspects {
            formatting: true,
            ..Default::default()
        }
    );
    let wrong: Vec<Entry> = entries(&diff)
        .into_iter()
        .filter(|entry| entry.0 != "container" && (entry.0 != "modified" || entry.6 != formatting))
        .collect();
    let top_wrong = diff
        .top_level
        .as_ref()
        .is_some_and(|top| format!("{:?}", top.aspects) != formatting);
    (!wrong.is_empty() || top_wrong).then(|| format!("{wrong:?} top {:?}", diff.top_level))
}

/// One changed definition, flattened, in a form its inverse can be
/// compared with: `(status, old name, new name, old place, new place,
/// lines, aspects)`.
type Entry = (
    &'static str,
    Option<String>,
    Option<String>,
    Option<(usize, usize, Option<usize>)>,
    Option<(usize, usize, Option<usize>)>,
    (usize, usize),
    String,
);

fn entries(diff: &DefinitionDiff) -> Vec<Entry> {
    fn walk(change: &DefinitionChange, out: &mut Vec<Entry>) {
        let place = |place: &Option<deltoids::Place>| {
            place
                .as_ref()
                .map(|place| (*place.lines.start(), *place.lines.end(), place.first_change))
        };
        let (status, old_name) = match &change.status {
            Status::Added => ("added", None),
            Status::Removed => ("removed", Some(change.name.clone())),
            Status::Modified => ("modified", Some(change.name.clone())),
            Status::Renamed { from } => ("renamed", Some(from.clone())),
            Status::Moved { from: Some(_) } => ("moved", Some(change.name.clone())),
            Status::Moved { from: None } => ("reordered", Some(change.name.clone())),
            Status::Container => ("container", Some(change.name.clone())),
        };
        let new_name = (!matches!(change.status, Status::Removed)).then(|| change.name.clone());
        out.push((
            status,
            old_name,
            new_name,
            place(&change.old),
            place(&change.new),
            change.lines,
            format!("{:?}", change.aspects),
        ));
        for child in &change.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    for change in &diff.changes {
        walk(change, &mut out);
    }
    out.sort();
    out
}

fn inverse(entry: Entry) -> Entry {
    let (status, old_name, new_name, old, new, (added, removed), aspects) = entry;
    let status = match status {
        "added" => "removed",
        "removed" => "added",
        other => other,
    };
    (
        status,
        new_name,
        old_name,
        new,
        old,
        (removed, added),
        aspects,
    )
}

/// Changed lines of the diff: (removed old lines, added new lines), 1-based.
fn changed_lines(diff: &Diff) -> (Vec<usize>, Vec<usize>) {
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    for hunk in diff.hunks() {
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            match line.kind {
                deltoids::LineKind::Context => {
                    old += 1;
                    new += 1;
                }
                deltoids::LineKind::Removed => {
                    removed.push(old);
                    old += 1;
                }
                deltoids::LineKind::Added => {
                    added.push(new);
                    new += 1;
                }
            }
        }
    }
    (removed, added)
}

/// What the definition changes from `before` to `after` get wrong:
/// comparing a version with itself must report nothing, the reverse change
/// must report the inverse, every changed line with content must fall in a
/// reported change (or the top level must report one), and a second run
/// must agree with the first.
fn definition_violations(before: &str, after: &str, path: &str) -> Vec<String> {
    let Some(forward) = DefinitionDiff::compute(before, after, path) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    for text in [before, after] {
        if let Some(same) = DefinitionDiff::compute(text, text, path)
            && !same.is_empty()
        {
            errors.push(format!("a version compared with itself reports {same:?}"));
        }
    }
    if DefinitionDiff::compute(before, after, path).as_ref() != Some(&forward) {
        errors.push("two runs disagree".to_string());
    }
    // Over broken syntax the report is a best effort, and says so.
    if forward.has_errors {
        return errors;
    }
    let backward = DefinitionDiff::compute(after, before, path).expect("same language");
    let flipped: Vec<Entry> = {
        let mut flipped: Vec<Entry> = entries(&backward).into_iter().map(inverse).collect();
        flipped.sort();
        flipped
    };
    // The line diff may align a reversed change differently, so line
    // counts, first changes, and moves within a parent (which need the
    // line diff to rewrite every line) need not mirror; nor need the
    // containers listed only for them.
    let shape = |entries: Vec<Entry>| {
        let mut shape: Vec<_> = entries
            .into_iter()
            .filter(|entry| !matches!(entry.0, "reordered" | "container"))
            .map(|(status, old_name, new_name, old, new, _, aspects)| {
                let range =
                    |place: Option<(usize, usize, Option<usize>)>| place.map(|(a, b, _)| (a, b));
                (status, old_name, new_name, range(old), range(new), aspects)
            })
            .collect();
        shape.sort();
        shape
    };
    if shape(entries(&forward)) != shape(flipped.clone()) {
        errors.push(format!(
            "the reverse change is not the inverse:\n  forward  {:?}\n  backward {:?}",
            entries(&forward),
            flipped
        ));
    }
    let all = entries(&forward);
    let diff = Diff::compute(before, after, path);
    let (removed, added) = changed_lines(&diff);
    let has_content = |text: &str, line: usize| {
        text.lines()
            .nth(line - 1)
            .is_some_and(|line| line.chars().any(char::is_alphanumeric))
    };
    let covers = |line: usize, old_side: bool| {
        all.iter().any(|(_, _, _, old, new, _, _)| {
            let place = if old_side { old } else { new };
            place.is_some_and(|(start, end, _)| (start..=end).contains(&line))
        })
    };
    if forward.top_level.is_none() {
        for line in removed
            .into_iter()
            .filter(|&line| has_content(before, line))
        {
            if !covers(line, true) {
                errors.push(format!("removed line {line} is in no reported change"));
            }
        }
        for line in added.into_iter().filter(|&line| has_content(after, line)) {
            if !covers(line, false) {
                errors.push(format!("added line {line} is in no reported change"));
            }
        }
    }
    errors
}

/// Reveal a seeded new-file line no hunk draws: the result must draw it,
/// keep exact cover, and fold back to the original hunks with one shrink
/// of the hunk that now holds it. `None` when every line is drawn.
fn reveal_violation(original: &str, updated: &str, path: &str, seed: u64) -> Option<String> {
    let diff = Diff::compute(original, updated, path);
    let drawn = drawn_new_lines(diff.hunks());
    let hidden: Vec<usize> = (1..=updated.lines().count())
        .filter(|line| !drawn.contains(line))
        .collect();
    if hidden.is_empty() || diff.hunks().is_empty() {
        return None;
    }
    let line = hidden[seed as usize % hidden.len()];
    let Some(revealed) = diff.reveal(line) else {
        return Some(format!("reveal {line}: returned None"));
    };
    if let Some(error) = exact_cover::violations(original, updated, revealed.hunks()).first() {
        return Some(format!("reveal {line}: {error}"));
    }
    let Some(holder) = revealed
        .hunks()
        .iter()
        .position(|hunk| drawn_new_lines(std::slice::from_ref(hunk)).contains(&line))
    else {
        return Some(format!("reveal {line}: line still hidden"));
    };
    let folded = revealed.shrink(holder);
    if folded.map(|diff| format!("{:?}", diff.hunks())) != Some(format!("{:?}", diff.hunks())) {
        return Some(format!("reveal {line}: one shrink did not fold it back"));
    }
    None
}

/// 1-based new-file lines the hunks draw.
fn drawn_new_lines(hunks: &[deltoids::Hunk]) -> std::collections::HashSet<usize> {
    let mut drawn = std::collections::HashSet::new();
    for hunk in hunks {
        let mut line = hunk.new_start;
        for diff_line in &hunk.lines {
            if diff_line.kind != deltoids::LineKind::Removed {
                drawn.insert(line);
                line += 1;
            }
        }
    }
    drawn
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
