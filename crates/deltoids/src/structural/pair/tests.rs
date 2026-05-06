//! Tests for symbol pairing.

use super::*;
use crate::structural::symbol::extract_symbols;

fn count_kind(pairings: &[Pairing], pred: impl Fn(&Pairing) -> bool) -> usize {
    pairings.iter().filter(|p| pred(p)).count()
}

#[test]
fn matches_same_path() {
    let old = extract_symbols("a.rs", "fn hello() {}\n");
    let new = extract_symbols("a.rs", "fn hello() { println!(\"hi\"); }\n");
    let pairings = pair_symbols(old, new);
    assert_eq!(pairings.len(), 1);
    assert!(matches!(&pairings[0], Pairing::Match { .. }));
}

#[test]
fn detects_added_symbol() {
    let old = extract_symbols("a.rs", "fn hello() {}\n");
    let new = extract_symbols("a.rs", "fn hello() {}\nfn newer() {}\n");
    let pairings = pair_symbols(old, new);
    assert_eq!(pairings.len(), 2);
    assert_eq!(
        count_kind(&pairings, |p| matches!(p, Pairing::Match { .. })),
        1
    );
    assert_eq!(
        count_kind(&pairings, |p| matches!(p, Pairing::NewOnly(_))),
        1
    );
}

#[test]
fn detects_removed_symbol() {
    let old = extract_symbols("a.rs", "fn hello() {}\nfn old() {}\n");
    let new = extract_symbols("a.rs", "fn hello() {}\n");
    let pairings = pair_symbols(old, new);
    assert_eq!(pairings.len(), 2);
    assert_eq!(
        count_kind(&pairings, |p| matches!(p, Pairing::OldOnly(_))),
        1
    );
}

#[test]
fn detects_rename_when_signature_similar() {
    let old = extract_symbols(
        "a.rs",
        "pub fn compute_total(x: i32, y: i32) -> i32 { x + y }\n",
    );
    let new = extract_symbols(
        "a.rs",
        "pub fn calc_total(x: i32, y: i32) -> i32 { x + y }\n",
    );
    let pairings = pair_symbols(old, new);
    let renames = pairings
        .iter()
        .filter(|p| matches!(p, Pairing::Rename { .. }))
        .count();
    assert_eq!(renames, 1, "pairings: {pairings:?}");
}

#[test]
fn does_not_rename_across_kinds() {
    // A function and a class with similar names should never be paired.
    let old = extract_symbols("a.py", "def hello(): pass\n");
    let new = extract_symbols("a.py", "class hello:\n    pass\n");
    let pairings = pair_symbols(old, new);
    let renames = pairings
        .iter()
        .filter(|p| matches!(p, Pairing::Rename { .. }))
        .count();
    assert_eq!(renames, 0);
}

#[test]
fn nested_methods_match_independently() {
    let old = extract_symbols("a.rs", "impl Foo { pub fn one() {} pub fn two() {} }\n");
    let new = extract_symbols(
        "a.rs",
        "impl Foo { pub fn one() { /* tweak */ } pub fn two() {} }\n",
    );
    let pairings = pair_symbols(old, new);
    let matches = pairings
        .iter()
        .filter(|p| matches!(p, Pairing::Match { .. }))
        .count();
    assert_eq!(matches, 2);
}

#[test]
fn signature_similarity_is_one_for_identical() {
    assert_eq!(
        super::signature_similarity("pub fn x(a: i32) -> i32", "pub fn x(a: i32) -> i32"),
        1.0
    );
}

#[test]
fn signature_similarity_is_zero_for_no_overlap() {
    let s = super::signature_similarity("class Foo extends Bar", "interface Quux");
    assert!(s < 0.1, "similarity = {s}");
}

#[test]
fn rename_with_otherwise_orphan_old_pairs_first() {
    // Two new symbols both look similar to the same old symbol — the
    // greedy algorithm picks one. Any pair we make is fine; we just
    // shouldn't create more renames than the number of orphans on the
    // smaller side.
    let old = extract_symbols("a.rs", "pub fn alpha(x: i32) -> i32 { x }\n");
    let new = extract_symbols(
        "a.rs",
        "pub fn beta(x: i32) -> i32 { x }\npub fn gamma(x: i32) -> i32 { x }\n",
    );
    let pairings = pair_symbols(old, new);
    let renames = pairings
        .iter()
        .filter(|p| matches!(p, Pairing::Rename { .. }))
        .count();
    let new_only = pairings
        .iter()
        .filter(|p| matches!(p, Pairing::NewOnly(_)))
        .count();
    assert_eq!(renames, 1);
    assert_eq!(new_only, 1);
}
