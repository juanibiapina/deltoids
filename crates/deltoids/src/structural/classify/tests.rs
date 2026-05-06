//! Tests for classifier + descriptions.

use super::*;
use crate::structural::pair::pair_symbols;
use crate::structural::symbol::extract_symbols;

fn changes(old_src: &str, new_src: &str, path: &str) -> Vec<StructuralChange> {
    let old = extract_symbols(path, old_src);
    let new = extract_symbols(path, new_src);
    classify(pair_symbols(old, new))
}

#[test]
fn identical_files_produce_no_changes() {
    let c = changes("fn x() {}\n", "fn x() {}\n", "a.rs");
    assert_eq!(c.len(), 0, "{c:#?}");
}

#[test]
fn added_function_described_as_added_function() {
    let c = changes("fn x() {}\n", "fn x() {}\nfn y() {}\n", "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::Added);
    assert_eq!(c[0].description, "Added function `y`");
}

#[test]
fn removed_method_described_as_removed_method() {
    let old = "impl Foo { fn alpha(&self) {} fn beta(&self) {} }\n";
    let new = "impl Foo { fn alpha(&self) {} }\n";
    let c = changes(old, new, "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::Removed);
    assert_eq!(c[0].description, "Removed method `Foo::beta`");
}

#[test]
fn body_only_change_is_body_changed() {
    let old = "pub fn hello() {\n    1\n}\n";
    let new = "pub fn hello() {\n    1\n    + 2\n}\n";
    let c = changes(old, new, "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::BodyChanged);
    assert_eq!(c[0].description, "Modified function `hello`");
}

#[test]
fn signature_only_change_is_signature_changed() {
    let old = "pub fn add(a: i32) -> i32 {\n    a\n}\n";
    let new = "pub fn add(a: i32, b: i32) -> i32 {\n    a\n}\n";
    let c = changes(old, new, "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::SignatureChanged);
    assert!(
        c[0].description
            .contains("Changed signature of function `add`"),
        "{}",
        c[0].description
    );
}

#[test]
fn visibility_only_change_is_visibility_changed() {
    let old = "fn hello() {}\n";
    let new = "pub fn hello() {}\n";
    let c = changes(old, new, "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::VisibilityChanged);
    assert!(
        c[0].description.contains("private → public"),
        "{}",
        c[0].description
    );
}

#[test]
fn rename_described_as_renamed() {
    let old = "pub fn compute_total(x: i32, y: i32) -> i32 { x + y }\n";
    let new = "pub fn calc_total(x: i32, y: i32) -> i32 { x + y }\n";
    let c = changes(old, new, "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::Renamed);
    assert_eq!(
        c[0].description,
        "Renamed function `compute_total` → `calc_total`"
    );
}

#[test]
fn public_only_filter_hides_private_changes() {
    let old = "fn priv_a() {}\npub fn pub_a() {}\n";
    let new = "fn priv_a() { let x = 1; }\npub fn pub_a() { let y = 2; }\n";
    let c = changes(old, new, "a.rs");
    let public: Vec<_> = c.iter().filter(|ch| ch.is_public()).collect();
    assert_eq!(public.len(), 1);
    assert!(
        public[0].description.contains("pub_a"),
        "{}",
        public[0].description
    );
}

#[test]
fn added_public_symbol_includes_visibility_marker() {
    let c = changes("fn x() {}\n", "fn x() {}\npub fn y() {}\n", "a.rs");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].description, "Added function `y` (public)");
}
