//! Tests for the structural outline.

use super::*;

fn outline_at(path: &str, old: &str, new: &str) -> Vec<OutlineEntry> {
    outline(old, new, path)
}

fn names_status(o: &[OutlineEntry]) -> Vec<(String, OutlineStatus)> {
    o.iter().map(|e| (e.qualified_name(), e.status)).collect()
}

#[test]
fn empty_for_unsupported_language() {
    assert!(outline_at("data.unknown", "anything", "different").is_empty());
}

#[test]
fn unchanged_file_lists_every_symbol_as_unchanged() {
    let src = "fn one() {}\nfn two() {}\n";
    let o = outline_at("a.rs", src, src);
    assert_eq!(
        names_status(&o),
        vec![
            ("one".to_string(), OutlineStatus::Unchanged),
            ("two".to_string(), OutlineStatus::Unchanged),
        ]
    );
}

#[test]
fn added_symbol_marked_added_unchanged_neighbours_kept() {
    let old = "fn one() {}\nfn two() {}\n";
    let new = "fn one() {}\nfn middle() {}\nfn two() {}\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(
        names_status(&o),
        vec![
            ("one".to_string(), OutlineStatus::Unchanged),
            ("middle".to_string(), OutlineStatus::Added),
            ("two".to_string(), OutlineStatus::Unchanged),
        ]
    );
}

#[test]
fn removed_symbol_kept_in_outline_with_removed_status() {
    let old = "fn one() {}\nfn two() {}\nfn three() {}\n";
    let new = "fn one() {}\nfn three() {}\n";
    let o = outline_at("a.rs", old, new);
    let names: Vec<&str> = o.iter().map(|e| e.path[0].as_str()).collect();
    assert!(
        names.contains(&"two"),
        "expected `two` in outline: {names:?}"
    );
    let two = o.iter().find(|e| e.path == ["two"]).unwrap();
    assert_eq!(two.status, OutlineStatus::Removed);
    assert_eq!(two.new_line, None);
    assert_eq!(two.old_line, Some(2));
}

#[test]
fn body_change_marked_body_changed() {
    let old = "fn x() {\n    1\n}\n";
    let new = "fn x() {\n    1 + 2\n}\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].status, OutlineStatus::BodyChanged);
}

#[test]
fn signature_change_marked_signature_changed() {
    let old = "pub fn add(a: i32) -> i32 {\n    a\n}\n";
    let new = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o.len(), 1);
    // Signature + body both changed → Modified (catch-all).
    assert!(matches!(
        o[0].status,
        OutlineStatus::SignatureChanged | OutlineStatus::Modified
    ));
}

#[test]
fn visibility_change_marked_visibility_changed() {
    let old = "fn x() {}\n";
    let new = "pub fn x() {}\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].status, OutlineStatus::VisibilityChanged);
}

#[test]
fn rename_marked_renamed_with_old_path() {
    let old = "pub fn compute_total(x: i32, y: i32) -> i32 { x + y }\n";
    let new = "pub fn calc_total(x: i32, y: i32) -> i32 { x + y }\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].status, OutlineStatus::Renamed);
    assert_eq!(o[0].path, vec!["calc_total".to_string()]);
    assert_eq!(o[0].renamed_from, Some(vec!["compute_total".to_string()]));
}

#[test]
fn class_with_added_method_class_modified_method_added() {
    let old = "class Foo:\n    def one(self):\n        pass\n";
    let new = "\
class Foo:
    def one(self):
        pass
    def two(self):
        pass
";
    let o = outline_at("a.py", old, new);
    let by_path: std::collections::HashMap<_, _> =
        o.iter().map(|e| (e.qualified_name(), e.status)).collect();
    // The class itself is body-changed (its body_text gained a method).
    assert!(by_path.contains_key("Foo"));
    let foo_status = by_path["Foo"];
    assert!(
        matches!(
            foo_status,
            OutlineStatus::BodyChanged | OutlineStatus::Modified
        ),
        "got: {foo_status:?}"
    );
    // The new method is added.
    assert_eq!(by_path["Foo::two"], OutlineStatus::Added);
    // The pre-existing method is unchanged.
    assert_eq!(by_path["Foo::one"], OutlineStatus::Unchanged);
}

#[test]
fn depth_zero_for_top_level_one_for_methods() {
    let src = "\
class Foo:
    def bar(self):
        pass
def top():
    pass
";
    let o = outline_at("a.py", src, src);
    let foo = o.iter().find(|e| e.path == ["Foo"]).unwrap();
    let bar = o.iter().find(|e| e.path == ["Foo", "bar"]).unwrap();
    let top = o.iter().find(|e| e.path == ["top"]).unwrap();
    assert_eq!(foo.depth, 0);
    assert_eq!(bar.depth, 1);
    assert_eq!(top.depth, 0);
}

#[test]
fn outline_is_sorted_by_new_line_for_added_and_unchanged() {
    let old = "fn a() {}\nfn b() {}\n";
    let new = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
    let o = outline_at("a.rs", old, new);
    let names: Vec<&str> = o.iter().map(|e| e.path[0].as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c", "d"]);
}

#[test]
fn description_added_says_added() {
    let o = outline_at("a.rs", "", "fn x() {}\n");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].description(), "added");
}

#[test]
fn description_unchanged_is_empty() {
    let o = outline_at("a.rs", "fn x() {}\n", "fn x() {}\n");
    assert_eq!(o[0].description(), "");
}

#[test]
fn description_body_changed_specific() {
    let old = "fn x() {\n    1\n}\n";
    let new = "fn x() {\n    1 + 2\n}\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o[0].description(), "body changed");
}

#[test]
fn description_signature_changed_specific() {
    let old = "pub fn x(a: i32) -> i32 { a }\n";
    let new = "pub fn x(a: i32, b: i32) -> i32 { a }\n";
    let o = outline_at("a.rs", old, new);
    assert!(
        o[0].description() == "signature changed" || o[0].description() == "modified",
        "got {:?}",
        o[0].description()
    );
}

#[test]
fn description_visibility_now_public() {
    let o = outline_at("a.rs", "fn x() {}\n", "pub fn x() {}\n");
    assert_eq!(o[0].description(), "private → public");
}

#[test]
fn description_renamed_includes_old_path() {
    let old = "pub fn compute_total(x: i32, y: i32) -> i32 { x + y }\n";
    let new = "pub fn calc_total(x: i32, y: i32) -> i32 { x + y }\n";
    let o = outline_at("a.rs", old, new);
    assert_eq!(o[0].description(), "renamed (was compute_total)");
}

#[test]
fn is_change_returns_false_for_unchanged_only() {
    assert!(!OutlineStatus::Unchanged.is_change());
    for s in [
        OutlineStatus::Added,
        OutlineStatus::Removed,
        OutlineStatus::BodyChanged,
        OutlineStatus::SignatureChanged,
        OutlineStatus::VisibilityChanged,
        OutlineStatus::Modified,
        OutlineStatus::Renamed,
    ] {
        assert!(s.is_change(), "expected {s:?} to count as a change");
    }
}
