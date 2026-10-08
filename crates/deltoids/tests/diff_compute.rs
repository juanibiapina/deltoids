//! `Diff::compute` behaviour through the public interface: hunk shape,
//! context expansion, and breadcrumbs. The diff cases under
//! `tests/diff_cases` are the broad spec; these pin narrow properties.

use deltoids::{Diff, DiffLine, Hunk, HunkRun, LineKind};

// -----------------------------------------------------------------------
// Diff::compute tests
// -----------------------------------------------------------------------

#[test]
fn compute_empty_returns_empty() {
    let diff = Diff::compute("", "", "test.rs");
    assert!(diff.hunks().is_empty());
}

// -----------------------------------------------------------------------
// Diff::expand tests
// -----------------------------------------------------------------------

fn old_spans(diff: &Diff) -> Vec<std::ops::Range<usize>> {
    diff.hunks()
        .iter()
        .map(|hunk| {
            let len = hunk
                .lines
                .iter()
                .filter(|line| line.kind != LineKind::Added)
                .count();
            hunk.old_start..hunk.old_start + len
        })
        .collect()
}

#[test]
fn expand_out_of_range_hunk_is_none() {
    let diff = Diff::compute("a\nb\n", "a\nc\n", "notes.txt");
    assert!(diff.expand(1).is_none());
}

#[test]
fn expand_never_hides_lines_a_hunk_showed() {
    let original = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n";
    let updated = original
        .replace("    1", "    10")
        .replace("    3", "    30");
    let diff = Diff::compute(original, &updated, "lib.rs");
    let expanded = diff.expand(0).expect("a larger level exists");
    for before in old_spans(&diff) {
        assert!(
            old_spans(&expanded)
                .iter()
                .any(|after| after.start <= before.start && before.end <= after.end),
            "{before:?} is no longer shown"
        );
    }
}

#[test]
fn shrink_undoes_the_last_expansion() {
    let original = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n";
    let updated = original
        .replace("    1", "    10")
        .replace("    3", "    30");
    let diff = Diff::compute(original, &updated, "lib.rs");
    let back = diff.expand(0).unwrap().shrink(0).unwrap();
    assert_eq!(format!("{:?}", back.hunks()), format!("{:?}", diff.hunks()));
    assert!(back.shrink(0).is_none());
}

#[test]
fn expand_all_grows_every_hunk_like_expanding_each_one() {
    let (original, updated) = two_distant_changes();
    let diff = Diff::compute(&original, &updated, "notes.txt");
    assert_eq!(diff.hunks().len(), 2);
    let one_by_one = diff.expand(0).unwrap().expand(1).unwrap();
    let all = diff.expand_all().expect("every hunk can grow");
    assert_eq!(old_spans(&all), old_spans(&one_by_one));
}

#[test]
fn expand_all_is_none_once_the_whole_file_shows() {
    let (original, updated) = two_distant_changes();
    let mut diff = Diff::compute(&original, &updated, "notes.txt");
    for _ in 0..20 {
        let Some(next) = diff.expand_all() else {
            assert_eq!(old_spans(&diff), vec![1..201]);
            return;
        };
        diff = next;
    }
    panic!("expand_all never reached the whole file");
}

#[test]
fn shrink_all_undoes_expand_all() {
    let (original, updated) = two_distant_changes();
    let diff = Diff::compute(&original, &updated, "notes.txt");
    assert!(diff.shrink_all().is_none());
    let back = diff.expand_all().unwrap().shrink_all().unwrap();
    assert_eq!(format!("{:?}", back.hunks()), format!("{:?}", diff.hunks()));
    assert!(back.shrink_all().is_none());
}

/// A 200-line plain text file with changes on lines 10 and 190: far
/// enough apart that one expansion step keeps them in separate hunks.
fn two_distant_changes() -> (String, String) {
    let original: String = (1..=200).map(|n| format!("line {n}\n")).collect();
    let updated = original
        .replace("line 10\n", "line ten\n")
        .replace("line 190\n", "line one-ninety\n");
    (original, updated)
}

// -----------------------------------------------------------------------
// Hunk::runs tests
// -----------------------------------------------------------------------

fn dl(kind: LineKind, content: &str) -> DiffLine {
    DiffLine {
        kind,
        content: content.to_string(),
    }
}

#[test]
fn runs_groups_consecutive_change_lines_and_emits_context_singletons() {
    let hunk = Hunk {
        old_start: 1,
        new_start: 1,
        ancestors: Vec::new(),
        lines: vec![
            dl(LineKind::Context, "alpha"),
            dl(LineKind::Removed, "old1"),
            dl(LineKind::Removed, "old2"),
            dl(LineKind::Added, "new1"),
            dl(LineKind::Context, "beta"),
            dl(LineKind::Added, "new2"),
        ],
    };

    let runs: Vec<HunkRun<'_>> = hunk.runs().collect();

    assert_eq!(runs.len(), 4);
    match &runs[0] {
        HunkRun::Context(line) => assert_eq!(line.content, "alpha"),
        other => panic!("expected Context, got {other:?}"),
    }
    match &runs[1] {
        HunkRun::Change(slice) => {
            assert_eq!(slice.len(), 3);
            assert_eq!(slice[0].content, "old1");
            assert_eq!(slice[1].content, "old2");
            assert_eq!(slice[2].content, "new1");
        }
        other => panic!("expected Change, got {other:?}"),
    }
    match &runs[2] {
        HunkRun::Context(line) => assert_eq!(line.content, "beta"),
        other => panic!("expected Context, got {other:?}"),
    }
    match &runs[3] {
        HunkRun::Change(slice) => {
            assert_eq!(slice.len(), 1);
            assert_eq!(slice[0].content, "new2");
        }
        other => panic!("expected Change, got {other:?}"),
    }
}

#[test]
fn runs_empty_hunk_yields_no_runs() {
    let hunk = Hunk {
        old_start: 1,
        new_start: 1,
        ancestors: Vec::new(),
        lines: Vec::new(),
    };
    assert_eq!(hunk.runs().count(), 0);
}

#[test]
fn compute_single_added_line() {
    let original = "line1\n";
    let updated = "line1\nline2\n";
    let diff = Diff::compute(original, updated, "test.txt");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].old_start, 1);
    assert_eq!(hunks[0].new_start, 1);
    assert_eq!(hunks[0].lines.len(), 2);
    assert_eq!(hunks[0].lines[0].kind, LineKind::Context);
    assert_eq!(hunks[0].lines[0].content, "line1");
    assert_eq!(hunks[0].lines[1].kind, LineKind::Added);
    assert_eq!(hunks[0].lines[1].content, "line2");
}

#[test]
fn compute_populates_ancestors_for_rust() {
    let original = "\
fn compute() {
    let x = 1;
    let y = 2;
}
";
    let updated = original.replace("let x = 1", "let x = 10");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].kind, "function_item");
    assert_eq!(hunks[0].ancestors[0].name, "compute");
}

#[test]
fn compute_nested_scope_impl_and_function() {
    let original = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
    let updated = original.replace("x + 1", "x + 2");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].ancestors.len(), 2);
    assert_eq!(hunks[0].ancestors[0].kind, "impl_item");
    assert_eq!(hunks[0].ancestors[0].name, "Foo");
    assert_eq!(hunks[0].ancestors[1].kind, "function_item");
    assert_eq!(hunks[0].ancestors[1].name, "compute");
}

#[test]
fn diff_reports_language_detected_from_shebang() {
    let original = "#!/usr/bin/env python3\n\ndef run():\n    return 1\n";
    let updated = "#!/usr/bin/env python3\n\ndef run():\n    return 2\n";

    let diff = Diff::compute(original, updated, "script");

    assert_eq!(
        diff.language().map(|language| language.id()),
        Some("python")
    );
}

#[test]
fn diff_uses_detected_language_when_updated_lacks_shebang() {
    let original = "#!/usr/bin/env python3\n\ndef run():\n    return 1\n";
    let updated = "def run():\n    return 2\n";

    let diff = Diff::compute(original, updated, "script");

    assert_eq!(
        diff.language().map(|language| language.id()),
        Some("python")
    );
    assert!(
        diff.hunks()
            .iter()
            .flat_map(|hunk| &hunk.ancestors)
            .any(|scope| scope.kind == "function_definition")
    );
}

#[test]
fn compute_unsupported_language_empty_ancestors() {
    let original = "line1\nline2\nline3\n";
    let updated = "line1\nLINE2\nline3\n";
    let diff = Diff::compute(original, updated, "data.xyz");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert!(hunks[0].ancestors.is_empty());
}

#[test]
fn highlight_uses_syntect_name_for_supported_language() {
    let diff = Diff::compute("let x = 1;\n", "let x = 2;\n", "test.rs");
    assert_eq!(diff.highlight(), Some("Rust"));
}

#[test]
fn highlight_detects_dockerfile_without_tree_sitter_support() {
    // Dockerfile has no tree-sitter `Language` but still highlights.
    let original = "FROM rust:1\nRUN cargo build\n";
    let updated = "FROM rust:1\nRUN cargo test\n";
    let diff = Diff::compute(original, updated, "Dockerfile");
    assert_eq!(diff.language(), None);
    assert_eq!(diff.highlight(), Some("Dockerfile"));
}

#[test]
fn highlight_detects_dockerfile_from_first_line() {
    let original = "FROM rust:1\nRUN cargo build\n";
    let updated = "FROM rust:1\nRUN cargo test\n";
    let diff = Diff::compute(original, updated, "Dockerfile.prod");
    assert_eq!(diff.highlight(), Some("Dockerfile"));
}

#[test]
fn highlight_is_none_for_unrecognised_file() {
    let diff = Diff::compute("line1\n", "line2\n", "data.unknownext");
    assert_eq!(diff.highlight(), None);
}

#[test]
fn compute_top_level_code_empty_ancestors() {
    let original = "let x = 1;\nlet y = 2;\n";
    let updated = "let x = 1;\nlet y = 3;\n";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert!(hunks[0].ancestors.is_empty());
}

#[test]
fn compute_json_hunk_has_no_ancestors() {
    // JSON has no named code structures, only data containers. The
    // breadcrumb chain should therefore be empty.
    let original = r#"{
  "scripts": {
    "build": "tsc",
    "test": "jest"
  }
}
"#;
    let updated = original.replace("\"jest\"", "\"vitest\"");
    let diff = Diff::compute(original, &updated, "package.json");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert!(
        hunks[0].ancestors.is_empty(),
        "JSON hunk should have no ancestors, got {:?}",
        hunks[0].ancestors
    );
}

#[test]
fn compute_typescript_config_hunk_has_no_ancestors() {
    // TypeScript config files use nested object literals with no
    // enclosing function or class. Data containers (object/array) do not
    // appear in the ancestor chain, so the breadcrumb is empty.
    let original = r#"export default defineConfig({
  env: {
    schema: {
      PUBLIC_KEY: "value1",
    },
  },
});
"#;
    let updated = original.replace("\"value1\"", "\"value2\"");
    let diff = Diff::compute(original, &updated, "astro.config.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert!(
        hunks[0].ancestors.is_empty(),
        "TS config hunk should have no structure ancestors, got {:?}",
        hunks[0].ancestors
    );
}

#[test]
fn compute_pair_inside_function_shows_function_ancestor() {
    // When a change is inside an object literal within a function,
    // the function should appear in ancestors (not just the pair)
    let original = r#"function getConfig() {
  return {
    env: {
      key: "value1",
    },
  };
}
"#;
    let updated = original.replace("\"value1\"", "\"value2\"");
    let diff = Diff::compute(original, &updated, "config.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Function should be in ancestors
    let has_function = hunks[0]
        .ancestors
        .iter()
        .any(|a| a.kind == "function_declaration");
    assert!(
        has_function,
        "Function should appear in ancestors when change is inside object within function"
    );
}

#[test]
fn compute_json_add_sibling_pair_keeps_both_added_lines() {
    // Regression: adding a trailing comma to a pair and a new sibling
    // pair on the next line dropped the new line from the hunk.
    let original = "{\n  \"deps\": {\n    \"a\": 1\n  }\n}\n";
    let updated = "{\n  \"deps\": {\n    \"a\": 1,\n    \"b\": 2\n  }\n}\n";

    let diff = Diff::compute(original, updated, "test.json");
    let added: Vec<&str> = diff
        .hunks()
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.as_str())
        .collect();

    assert!(
        added.iter().any(|l| l.contains("\"b\": 2")),
        "new sibling pair line should appear as added, got: {:?}",
        added
    );
}

#[test]
fn compute_yaml_add_sibling_pair_keeps_both_added_lines() {
    // Same bug as the JSON case, exercised through tree-sitter-yaml's
    // `block_mapping_pair` leaf container. Modifying a value alongside
    // the insertion forces the Replace code path (not a pure Insert).
    let original = "deps:\n  a: 1\n";
    let updated = "deps:\n  a: 2\n  b: 3\n";

    let diff = Diff::compute(original, updated, "test.yaml");
    let added: Vec<&str> = diff
        .hunks()
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.as_str())
        .collect();

    assert!(
        added.iter().any(|l| l.contains("b: 3")),
        "new sibling mapping pair line should appear as added, got: {:?}",
        added
    );
}

#[test]
fn compute_json_top_level_replace_keeps_all_added_lines() {
    // Regression: top-level JSON pairs are leaf containers with no parent
    // scope. A Replace spanning two pairs previously lost added lines
    // because the new-scope cutoff treated each pair as a fresh scope.
    let original = "\
{
  \"version\": \"1.0\",
  \"theme\": \"light\",
  \"model\": \"a\",
  \"thinking\": \"low\"
}
";
    let updated = "\
{
  \"version\": \"2.0\",
  \"theme\": \"light\",
  \"model\": \"b\",
  \"thinking\": \"high\"
}
";

    let diff = Diff::compute(original, updated, "settings.json");
    let added: Vec<&str> = diff
        .hunks()
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.as_str())
        .collect();

    assert!(
        added.iter().any(|l| l.contains("\"2.0\"")),
        "updated version should appear as added, got: {:?}",
        added
    );
    assert!(
        added.iter().any(|l| l.contains("\"b\"")),
        "updated model should appear as added, got: {:?}",
        added
    );
    assert!(
        added.iter().any(|l| l.contains("\"high\"")),
        "updated thinking level should appear as added, got: {:?}",
        added
    );
}

#[test]
fn compute_small_json_with_distant_changes_merges_into_one_hunk() {
    // Small file with changes split across the top and the bottom should
    // merge into a single hunk. The root object (< MAX_SCOPE_LINES) is
    // the outermost-fit container for both changes, so they share
    // the same anchored range.
    let original = "\
{
  \"a\": 1,
  \"b\": 2,
  \"c\": 3,
  \"d\": 4,
  \"e\": 5,
  \"f\": 6,
  \"g\": 7,
  \"h\": 8
}
";
    let updated = original
        .replace("\"a\": 1", "\"a\": 10")
        .replace("\"h\": 8", "\"h\": 80");

    let diff = Diff::compute(original, &updated, "small.json");
    assert_eq!(
        diff.hunks().len(),
        1,
        "small JSON with two edits should render as one hunk"
    );
}

#[test]
fn compute_large_json_falls_back_to_default_context() {
    // Root object spans over MAX_SCOPE_LINES (200); outermost-fit must
    // skip it (doesn't fit under the cap) and fall back to default
    // 3-line context rather than emitting a massive hunk.
    let mut lines: Vec<String> = Vec::new();
    lines.push("{".to_string());
    for i in 1..=210 {
        lines.push(format!("  \"k{i}\": {i},"));
    }
    lines.push("  \"last\": 0".to_string());
    lines.push("}".to_string());
    let original = lines.join("\n") + "\n";
    let updated = original.replace("\"k100\": 100", "\"k100\": 1000");

    let diff = Diff::compute(&original, &updated, "big.json");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // 3 before + 1 removed + 1 added + 3 after = at most 8 lines.
    assert!(
        hunks[0].lines.len() <= 8,
        "large JSON should use default context, got {} lines",
        hunks[0].lines.len()
    );
}

#[test]
fn compute_ts_change_in_object_inside_function_anchors_on_function() {
    // When a change is inside an object nested in a function, the hunk
    // anchors on the function (innermost structure wins over outermost
    // data). The function's name also appears in the ancestor chain.
    let original = "\
function getConfig() {
  const inner = {
    aaa: 1,
    bbb: 2,
  };
  return inner;
}
";
    let updated = original.replace("aaa: 1", "aaa: 10");

    let diff = Diff::compute(original, &updated, "config.ts");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1);
    let has_function = hunks[0]
        .ancestors
        .iter()
        .any(|a| a.kind == "function_declaration" && a.name == "getConfig");
    assert!(
        has_function,
        "expected function_declaration ancestor, got {:?}",
        hunks[0].ancestors
    );
    // Must not contain any object/array/pair in the breadcrumb.
    let has_data_kind = hunks[0]
        .ancestors
        .iter()
        .any(|a| matches!(a.kind.as_str(), "object" | "array" | "pair"));
    assert!(
        !has_data_kind,
        "anonymous containers should be filtered out of the breadcrumb, got {:?}",
        hunks[0].ancestors
    );
}

#[test]
fn compute_ts_top_level_const_object_anchors_on_object() {
    // A change inside a top-level `const x = { ... }` should anchor the
    // hunk on the whole statement (the outermost-fit transparent
    // ancestor, == the object span here), not produce only 3 lines of
    // default context.
    let original = "\
const config = {
  aaa: 1,
  bbb: 2,
  ccc: 3,
  ddd: 4,
  eee: 5,
  fff: 6,
  ggg: 7,
  hhh: 8,
};
";
    let updated = original.replace("aaa: 1", "aaa: 10");

    let diff = Diff::compute(original, &updated, "config.ts");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1);
    assert!(
        hunks[0].lines.len() >= 11,
        "hunk should cover the whole object, got {} lines",
        hunks[0].lines.len()
    );
    assert!(
        hunks[0].ancestors.is_empty(),
        "no structure wraps a top-level const, expected empty ancestors"
    );
}

#[test]
fn compute_json_lone_deep_change_anchors_on_root_object() {
    // A change only inside a nested array of a small JSON should produce
    // a single hunk that covers the whole root object (outermost-fit).
    let original = "\
{
  \"aaa\": 1,
  \"bbb\": 2,
  \"ccc\": 3,
  \"ddd\": 4,
  \"items\": [
    1,
    2,
    3
  ],
  \"eee\": 5,
  \"fff\": 6,
  \"ggg\": 7,
  \"hhh\": 8
}
";
    let updated = original.replace("    1,\n", "    10,\n");

    let diff = Diff::compute(original, &updated, "config.json");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1, "expected exactly one hunk");
    // Whole root object is 16 lines. With outermost-fit strategy the
    // hunk must cover the whole object, not just the default 3-line
    // context (which would produce ~7 lines).
    assert!(
        hunks[0].lines.len() >= 14,
        "hunk should cover the whole root object, got {} lines",
        hunks[0].lines.len()
    );
}

// -----------------------------------------------------------------------
// Scope-expanded context tests
// -----------------------------------------------------------------------

#[test]
fn expanded_covers_full_small_function() {
    // A 10-line function (< 50 lines) should have full scope context
    let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
}
";
    let updated = original.replace("let d = 4", "let d = 40");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Hunk should start at line 1 (function start) and include all lines
    assert_eq!(hunks[0].old_start, 1);
    // Should have context lines from the whole function
    let context_count = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Context)
        .count();
    // 10 lines total, 1 changed = 9 context lines
    assert_eq!(context_count, 9, "should have full function as context");
}

#[test]
fn expanded_large_scope_uses_structure_context() {
    // A function > 200 lines should use 100-line context per side,
    // not the full function.
    let mut lines = vec!["fn big() {".to_string()];
    for i in 1..=205 {
        lines.push(format!("    let x{} = {};", i, i));
    }
    lines.push("}".to_string());
    let original = lines.join("\n") + "\n";
    let updated = original.replace("let x100 = 100", "let x100 = 1000");
    let diff = Diff::compute(&original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Should NOT start at line 1 (function start is line 1,
    // change is at line 101, 100 before = line 1 — so it does start at 1
    // for a 207-line function with the change in the middle).
    // But it should NOT include the entire function.
    let total = hunks[0].lines.len();
    assert!(
        total < 207,
        "should not include entire function, got {} lines",
        total
    );
    // With 100-line budget: ~100 before + 1 removed + 1 added + ~100 after
    // = ~202 lines. The function starts at line 1 and the change is at
    // line 101, so the 100-line-before reaches the function start.
    assert!(
        total >= 100,
        "should use ~100-line context, got {} lines",
        total
    );
    // Breadcrumb should still name the function.
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "big");
}

#[test]
fn expanded_top_level_uses_default() {
    // Top-level code with no scope should use 3-line default context
    let original = "\
let a = 1;
let b = 2;
let c = 3;
let d = 4;
let e = 5;
let f = 6;
let g = 7;
let h = 8;
let i = 9;
let j = 10;
";
    let updated = original.replace("let e = 5", "let e = 50");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Should not include all 10 lines:
    // 3 before + 1 removed + 1 added + 3 after = 8 lines max
    let total_lines = hunks[0].lines.len();
    assert!(
        total_lines <= 8,
        "top-level should use 3-line context, got {} lines",
        total_lines
    );
    assert!(total_lines < 10, "should not include all lines");
}

#[test]
fn expanded_unsupported_lang_uses_default() {
    // Unknown language should use 3-line default context
    let mut lines: Vec<String> = (1..=20).map(|i| format!("line{}", i)).collect();
    let original = lines.join("\n") + "\n";
    lines[9] = "CHANGED".to_string();
    let updated = lines.join("\n") + "\n";
    let diff = Diff::compute(&original, &updated, "data.xyz");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // 3 before + 1 removed + 1 added + 3 after = 8 lines
    let total_lines = hunks[0].lines.len();
    assert!(
        total_lines <= 8,
        "unsupported lang should use 3-line context, got {} lines",
        total_lines
    );
    assert!(total_lines < 20, "should not include all lines");
}

#[test]
fn expanded_two_functions_separate_hunks() {
    // Two changes in separate functions should produce 2 hunks
    let original = "\
fn first() {
    let a = 1;
}

fn second() {
    let b = 2;
}
";
    let updated = original
        .replace("let a = 1", "let a = 10")
        .replace("let b = 2", "let b = 20");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 2, "two functions should produce 2 hunks");
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "first");
    assert_eq!(hunks[1].ancestors.len(), 1);
    assert_eq!(hunks[1].ancestors[0].name, "second");
}

#[test]
fn expanded_same_function_merges() {
    // Two changes in the same function should produce 1 merged hunk
    let original = "\
fn compute() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
}
";
    let updated = original
        .replace("let a = 1", "let a = 10")
        .replace("let d = 4", "let d = 40");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(
        hunks.len(),
        1,
        "same function changes should merge to 1 hunk"
    );
    // Should have 2 changed lines (a and d)
    let removed = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .count();
    let added = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .count();
    assert_eq!(removed, 2, "should have 2 removed lines");
    assert_eq!(added, 2, "should have 2 added lines");
}

#[test]
fn text_returns_standard_3_line_context() {
    // text() should return standard 3-line context diff
    let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
}
";
    let updated = original.replace("let d = 4", "let d = 40");
    let diff = Diff::compute(original, &updated, "test.rs");
    let text = diff.text();
    // Body should NOT include line 1 (fn small()) - only 3-line context
    let lines: Vec<&str> = text.lines().collect();
    let has_fn_line_in_body = lines.iter().any(|l| l.contains("fn small()"));
    assert!(
        !has_fn_line_in_body,
        "body should use 3-line context, not full scope"
    );
}

#[test]
fn hunks_have_expanded_context() {
    // hunks() should have scope-expanded context
    let original = "\
fn small() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
}
";
    let updated = original.replace("let d = 4", "let d = 40");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Hunks should include full function (start at line 1)
    assert_eq!(hunks[0].old_start, 1, "hunk should start at function start");
    // First context line should be fn small() {
    assert!(
        hunks[0].lines[0].content.contains("fn small()"),
        "first context line should be function signature"
    );
}

// -----------------------------------------------------------------------
// Diff tests
// -----------------------------------------------------------------------

#[test]
fn diff_compute_produces_hunks_and_raw_text() {
    let original = "line1\nline2\n";
    let updated = "line1\nLINE2\n";
    let diff = Diff::compute(original, updated, "test.txt");

    assert_eq!(diff.hunks().len(), 1);
    assert!(diff.text().contains("-line2"));
    assert!(diff.text().contains("+LINE2"));
}

#[test]
fn hunks_nested_shows_innermost_ancestor() {
    let original = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
    let updated = original.replace("x + 1", "x + 2");
    let diff = Diff::compute(original, &updated, "test.rs");

    // Innermost ancestor should be function, not impl
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].ancestors.last().unwrap().name, "compute");
}

#[test]
fn diff_hunks_returns_enriched_data() {
    let original = "fn hello() {\n    println!(\"hi\");\n}\n";
    let updated = "fn hello() {\n    println!(\"bye\");\n}\n";
    let diff = Diff::compute(original, updated, "test.rs");

    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "hello");
}

// -----------------------------------------------------------------------
// Line-diff guided scope detection tests
// -----------------------------------------------------------------------
#[test]
fn new_function_minimal_context() {
    // Adding a complete new function should show only that function,
    // not pull in siblings from the parent scope.
    let original = "\
fn existing() {
    let x = 1;
}
";
    let updated = "\
fn existing() {
    let x = 1;
}

fn new_function() {
    let y = 2;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Should produce one hunk for the new function
    assert_eq!(hunks.len(), 1);

    // The hunk should be all additions (the new function)
    let added = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .count();
    assert_eq!(
        added, 4,
        "should have 4 added lines (blank + fn signature + body + close)"
    );

    // Context should NOT include the existing function
    let has_existing = hunks[0]
        .lines
        .iter()
        .any(|l| l.content.contains("existing"));
    assert!(
        !has_existing,
        "new function hunk should not include sibling"
    );

    // Ancestors should come from the NEW tree and include the new function
    assert!(!hunks[0].ancestors.is_empty());
    assert_eq!(hunks[0].ancestors[0].name, "new_function");
}

#[test]
fn new_method_in_impl_correct_scope() {
    // Adding a new method to an impl should show method scope, not entire impl.
    let original = "\
struct Foo;

impl Foo {
    fn existing(&self) {
        let x = 1;
    }
}
";
    let updated = "\
struct Foo;

impl Foo {
    fn existing(&self) {
        let x = 1;
    }

    fn new_method(&self) {
        let y = 2;
    }
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1);

    // Should not include the existing method as context
    let has_existing_body = hunks[0]
        .lines
        .iter()
        .any(|l| l.content.contains("let x = 1"));
    assert!(
        !has_existing_body,
        "new method should not include sibling method body"
    );

    // Ancestors should include impl and the new method
    assert!(!hunks[0].ancestors.is_empty());
    let has_new_method = hunks[0].ancestors.iter().any(|a| a.name == "new_method");
    assert!(has_new_method, "ancestors should include new_method");
}

#[test]
fn deleted_function_minimal_context() {
    // Deleting a complete function should show only that function.
    let original = "\
fn first() {
    let a = 1;
}

fn to_delete() {
    let b = 2;
}

fn third() {
    let c = 3;
}
";
    let updated = "\
fn first() {
    let a = 1;
}

fn third() {
    let c = 3;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1);

    // Should only have the deleted function lines
    let removed = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .count();
    // 5 lines: blank + fn signature + body + close + blank
    assert!(removed >= 4, "should have deleted function lines");

    // Should not include first() or third() body as context
    let has_first_body = hunks[0]
        .lines
        .iter()
        .any(|l| l.content.contains("let a = 1"));
    let has_third_body = hunks[0]
        .lines
        .iter()
        .any(|l| l.content.contains("let c = 3"));
    assert!(!has_first_body, "should not include first() body");
    assert!(!has_third_body, "should not include third() body");

    // Ancestors should be from the OLD tree and include to_delete
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "to_delete");
}

#[test]
fn insert_inside_function_expands() {
    // Adding lines inside an existing function should expand to function scope.
    let original = "\
fn compute() {
    let a = 1;
    let c = 3;
}
";
    let updated = "\
fn compute() {
    let a = 1;
    let b = 2;
    let c = 3;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1);

    // Should include the function signature as context
    let has_fn_sig = hunks[0]
        .lines
        .iter()
        .any(|l| l.content.contains("fn compute()"));
    assert!(has_fn_sig, "should expand to include function signature");

    // Should include closing brace as context
    let has_close = hunks[0].lines.iter().any(|l| l.content.trim() == "}");
    assert!(has_close, "should include closing brace");

    // Ancestors should reference the function
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "compute");
}

#[test]
fn multiple_changes_same_function_merge() {
    // Three changes in one function should merge into one hunk.
    let original = "\
fn process() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
}
";
    let updated = original
        .replace("let a = 1", "let a = 10")
        .replace("let c = 3", "let c = 30")
        .replace("let e = 5", "let e = 50");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(
        hunks.len(),
        1,
        "three changes in same function should produce 1 hunk"
    );

    // All three changes should be in this hunk
    let removed = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .count();
    let added = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .count();
    assert_eq!(removed, 3, "should have 3 removed lines");
    assert_eq!(added, 3, "should have 3 added lines");
}

#[test]
fn modify_and_append_fused_replace_renders_faithfully() {
    // Modifying a line in `existing()` and appending a new function
    // fuses into one `Replace` op (the trailing `}` matches as
    // context). git renders this as a single hunk with +5/-1, and so
    // must deltoids: the builder renders every op line by its kind, so
    // displayed line counts match git.
    let original = "\
fn existing() {
    let x = 1;
}
";
    let updated = "\
fn existing() {
    let x = 10;
}

fn new_function() {
    let y = 2;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1, "fused Replace renders as one hunk");
    let added = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .count();
    let removed = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .count();
    assert_eq!((added, removed), (5, 1), "line counts match git (+5 -1)");
}

#[test]
fn replace_with_add_and_delete_same_function_single_hunk() {
    // Bug: When a Replace operation adds some lines and removes others
    // within the same function, it was creating duplicate hunks with
    // the same scope header.
    //
    // This reproduces the bug seen in `git show | deltoids` where
    // process_diff appeared twice.
    let original = "\
fn process() {
    let a = 1;
    // old comment
    let b = 2;
    let c = 3;
}
";
    // Change: remove "old comment", add "new comment" in different place
    let updated = "\
fn process() {
    let a = 1;
    // new comment
    let b = 2;
    let c = 3;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Should produce exactly ONE hunk, not two
    assert_eq!(
        hunks.len(),
        1,
        "modifications within same function should produce 1 hunk, got {}",
        hunks.len()
    );

    // The single hunk should have the function as ancestor
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "process");
}

#[test]
fn two_distant_changes_same_function_no_duplicate_headers() {
    // Bug reproduction: Two separate changes in the same function,
    // far enough apart to be separate git hunks, but within the same
    // scope. The second change has both additions and deletions.
    //
    // This matches the pattern in `git show HEAD` for main.rs where
    // process_diff header appeared twice.
    let original = "\
fn process_diff() {
    // Section 1
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    
    // Section 2
    let i = 9;
    // old comment to remove
    let j = 10;
    let k = 11;
}
";
    let updated = "\
fn process_diff() {
    // Section 1
    let a = 1;
    // NEW: added line in section 1
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    
    // Section 2
    let i = 9;
    let j = 10;
    // NEW: added line in section 2
    let k = 11;
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Should produce exactly ONE hunk (both changes are in same function scope)
    assert_eq!(
        hunks.len(),
        1,
        "two changes in same function should merge to 1 hunk, got {}",
        hunks.len()
    );

    // Verify the hunk has the correct ancestor
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "process_diff");
}

#[test]
fn rename_function_produces_single_hunk() {
    // Bug: When renaming a function (same location, different name),
    // the code treats the new function as a "new scope" and creates
    // two separate hunks instead of one merged hunk.
    //
    // This reproduces a bug seen in an edit to render.rs
    // where `fn theme()` was renamed to `fn syntect_theme()` and
    // produced two separate hunks with different ancestors.
    let original = "\
fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_theme(THEME_NAME)
            .clone()
    })
}
";
    let updated = "\
fn syntect_theme() -> &'static SyntectTheme {
    SYNTECT_THEME.get_or_init(|| {
        HighlightingAssets::from_binary()
            .get_theme(THEME_NAME)
            .clone()
    })
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    // Bug: Currently produces 2 hunks when it should produce 1.
    // First hunk has ancestor "theme" (old), second has "syntect_theme" (new).
    assert_eq!(
        hunks.len(),
        1,
        "renaming a function should produce 1 hunk, not {}; \
             hunks have ancestors: {:?}",
        hunks.len(),
        hunks.iter().map(|h| &h.ancestors).collect::<Vec<_>>()
    );

    // The single hunk should have changes (not be context-only)
    let has_removed = hunks[0].lines.iter().any(|l| l.kind == LineKind::Removed);
    let has_added = hunks[0].lines.iter().any(|l| l.kind == LineKind::Added);
    assert!(has_removed, "hunk should have removed lines");
    assert!(has_added, "hunk should have added lines");

    // The ancestor should be from one of the functions (either old or new is fine)
    assert!(!hunks[0].ancestors.is_empty(), "hunk should have ancestors");
    let ancestor_name = &hunks[0].ancestors[0].name;
    assert!(
        ancestor_name == "theme" || ancestor_name == "syntect_theme",
        "ancestor should be either 'theme' or 'syntect_theme', got '{}'",
        ancestor_name
    );
}

#[test]
fn insert_at_end_of_file_without_scope() {
    // Bug: inserting at end of a file with no enclosing scope produced
    // empty diff. The Insert op has old_index past the last line, but
    // range.end is clamped to total_old - 1, so the insert was skipped.
    // Use .ts to trigger tree-sitter parsing (unlike .txt which bypasses it).
    let original = "const a = 1;\nconst b = 2;\nconst c = 3;\nconst d = 4;\nconst e = 5;\nconst f = 6;\nconst g = 7;\n";
    let updated = "const a = 1;\nconst b = 2;\nconst c = 3;\nconst d = 4;\nconst e = 5;\nconst f = 6;\nconst g = 7;\nconst h = 8;\n";
    let diff = Diff::compute(original, updated, "test.ts");
    let hunks = diff.hunks();

    assert_eq!(hunks.len(), 1, "should produce 1 hunk");
    let added: Vec<_> = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].content, "const h = 8;");
}

#[test]
fn replace_function_signature_does_not_create_context_only_hunk() {
    let original = "\
fn default_context_range(old_start: usize, old_end: usize, total_old: usize) -> ContextRange {
    let start = old_start.saturating_sub(DEFAULT_CONTEXT);
    let end = (old_end + DEFAULT_CONTEXT).min(total_old.saturating_sub(1));
    ContextRange { start, end }
}
";
    let updated = "\
fn default_context_range(
    old_start: usize,
    old_end: usize,
    total_old: usize,
    ancestor_source: AncestorSource,
    scope_line: usize,
) -> ContextRange {
    let start = old_start.saturating_sub(DEFAULT_CONTEXT);
    let end = (old_end + DEFAULT_CONTEXT).min(total_old.saturating_sub(1));
    ContextRange {
        start,
        end,
        ancestor_source,
        scope_line,
        prevent_merge: false,
    }
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();

    assert_eq!(
        hunks.len(),
        1,
        "should not create a context-only sibling hunk"
    );
    assert!(hunks[0].lines.iter().any(|l| l.kind != LineKind::Context));
    assert_eq!(hunks[0].ancestors.len(), 1);
    assert_eq!(hunks[0].ancestors[0].name, "default_context_range");
}

// -----------------------------------------------------------------------
// Reviewer-grouping fixes (Fix A: diff-aware same-slot, Fix B: promoted
// structures for class fields and lexical declarations whose value is a
// function-like expression).
// -----------------------------------------------------------------------

#[test]
fn rename_after_prior_insert_produces_single_hunk() {
    // Cause 1: a rename happens after an earlier edit that shifted the
    // function down. The rename must not be classified as a "new scope"
    // just because absolute line positions differ between OLD and NEW.
    let original = "\
fn first() -> i32 { 1 }

fn target() -> i32 {
    42
}
";
    let updated = "\
fn first() -> i32 { 1 }
fn inserted() -> i32 { 0 }

fn renamed_target() -> i32 {
    42
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();
    let renamed = hunks
        .iter()
        .filter(|h| {
            h.ancestors
                .iter()
                .any(|a| a.name == "target" || a.name == "renamed_target")
        })
        .count();
    assert_eq!(
        renamed,
        1,
        "rename must not produce duplicate hunks; got {} hunks total: {:?}",
        hunks.len(),
        hunks
            .iter()
            .map(|h| h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn ts_three_consecutive_renames_after_import_deletion_stay_deduplicated() {
    // Mirrors the real-world case: an import is deleted at the top
    // of the file (shifting everything below) and three sibling
    // methods are each renamed. Each rename must produce exactly one
    // hunk; none should duplicate as a NEW-anchored extra.
    let original = "\
import { A } from './a';
import { B } from './b';

export class S {
  oldOne(): number {
    return 1;
  }

  oldTwo(): number {
    return 2;
  }

  oldThree(): number {
    return 3;
  }
}
";
    let updated = "\
import { A } from './a';

export class S {
  newOne(): number {
    return 1;
  }

  newTwo(): number {
    return 2;
  }

  newThree(): number {
    return 3;
  }
}
";
    let diff = Diff::compute(original, updated, "s.ts");
    let hunks = diff.hunks();
    for (old_name, new_name) in [
        ("oldOne", "newOne"),
        ("oldTwo", "newTwo"),
        ("oldThree", "newThree"),
    ] {
        let count = hunks
            .iter()
            .filter(|h| {
                h.ancestors
                    .iter()
                    .any(|a| a.name == old_name || a.name == new_name)
            })
            .count();
        assert_eq!(
            count,
            1,
            "{} -> {} must produce exactly one hunk, got {}; ancestors: {:?}",
            old_name,
            new_name,
            count,
            hunks
                .iter()
                .map(|h| h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn ts_rename_after_import_deletion_no_duplicate() {
    // Cause 1, TypeScript variant. Removing an import shifts every
    // method's line numbers; a method rename below the imports must
    // still be a single hunk.
    let original = "\
import { A } from './a';
import { B } from './b';

export class S {
  oldName(): number {
    return 1;
  }
}
";
    let updated = "\
import { A } from './a';

export class S {
  newName(): number {
    return 1;
  }
}
";
    let diff = Diff::compute(original, updated, "s.ts");
    let hunks = diff.hunks();
    let rename_hunks = hunks
        .iter()
        .filter(|h| {
            h.ancestors
                .iter()
                .any(|a| a.name == "oldName" || a.name == "newName")
        })
        .count();
    assert_eq!(
        rename_hunks,
        1,
        "rename should produce exactly one hunk; ancestors: {:?}",
        hunks
            .iter()
            .map(|h| h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn ts_class_arrow_field_appears_in_breadcrumb() {
    // Cause 2: a class field whose value is an arrow function should be
    // recognised as a structure so changes inside it anchor on the field.
    let original = "\
export class S {
  doWork = async (): Promise<number> => {
    const x = 1;
    return x;
  };
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork"),
        "breadcrumb should include 'doWork', got {:?}",
        names
    );
}

#[test]
fn ts_top_level_const_arrow_appears_in_breadcrumb() {
    // Cause 2 at module scope: `const f = () => {}` should anchor on
    // the variable name.
    let original = "\
export const compute = (): number => {
  const x = 1;
  return x;
};
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "compute.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"compute"),
        "breadcrumb should include 'compute', got {:?}",
        names
    );
}

#[test]
fn ts_arrow_field_to_method_conversion_is_single_hunk() {
    // Combination of Cause 1 + Cause 2: the field has a different shape
    // in OLD (arrow property) vs NEW (method). With both fixes it's
    // recognised as the same logical slot and produces one hunk.
    let original = "\
export class S {
  doWork = async (): Promise<void> => {
    return;
  };
}
";
    let updated = "\
export class S {
  async doWork(): Promise<void> {
    return;
  }
}
";
    let diff = Diff::compute(original, updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(
        hunks.len(),
        1,
        "arrow->method conversion of the same member should be one hunk; got {}",
        hunks.len()
    );
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork"),
        "breadcrumb should include 'doWork', got {:?}",
        names
    );
}

#[test]
fn anonymous_arrow_callback_not_promoted_to_structure() {
    // Negative test for Fix B: anonymous arrows passed as callbacks
    // must NOT become structures. The breadcrumb should remain on the
    // enclosing named function.
    let original = "\
function outer(items: number[]): number[] {
    return items.map((item) => {
        return item + 1;
    });
}
";
    let updated = original.replace("item + 1", "item + 2");
    let diff = Diff::compute(original, &updated, "x.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["outer"],
        "anonymous arrow must not appear in breadcrumb"
    );
}

#[test]
fn ts_short_non_function_field_not_promoted_to_structure() {
    // Negative test for Fix B: a class field with a non-function value
    // (e.g. `count = 0`) must NOT be treated as a structure.
    let original = "\
export class S {
  count = 0;
  doWork(): number {
    return this.count + 1;
  }
}
";
    let updated = original.replace("this.count + 1", "this.count + 2");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork") && !names.contains(&"count"),
        "expected anchor on doWork without 'count', got {:?}",
        names
    );
}

#[test]
fn brand_new_method_keeps_separate_hunk() {
    // Regression for Fix A: a brand-new method appended to a class
    // must still receive its own hunk.
    let original = "\
export class S {
  existing(): number { return 1; }
}
";
    let updated = "\
export class S {
  existing(): number { return 1; }
  added(): number { return 2; }
}
";
    let diff = Diff::compute(original, updated, "s.ts");
    let hunks = diff.hunks();
    assert!(
        hunks
            .iter()
            .any(|h| h.ancestors.iter().any(|a| a.name == "added")),
        "inserted method must have a hunk anchored on it; got {:?}",
        hunks
            .iter()
            .map(|h| h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn adjacent_methods_no_blank_line_stay_separate() {
    // Two physically adjacent methods (no blank line between them) with
    // independent edits must produce two hunks, not a merged one with a
    // misleading breadcrumb.
    let original = "\
export class S {
  alpha(): number {
    return 1;
  }
  beta(): number {
    return 2;
  }
}
";
    let updated = original
        .replace("return 1", "return 10")
        .replace("return 2", "return 20");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(
        hunks.len(),
        2,
        "edits in two adjacent methods must produce 2 hunks; got {}",
        hunks.len()
    );
    let names: Vec<Vec<&str>> = hunks
        .iter()
        .map(|h| h.ancestors.iter().map(|a| a.name.as_str()).collect())
        .collect();
    assert!(names.iter().any(|n| n.contains(&"alpha")));
    assert!(names.iter().any(|n| n.contains(&"beta")));
}

#[test]
fn nested_function_anchors_on_outer_function() {
    // A change inside a function nested in another function body must
    // anchor on the OUTER function. The reviewer's mental anchor is the
    // top-level named container (function, method, class member), not
    // local helper functions defined inline in the body.
    let original = "\
fn outer() {
    let x = 1;
    fn inner() {
        let y = 2;
    }
    let z = 3;
}
";
    let updated = original.replace("let y = 2", "let y = 20");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"outer") && !names.contains(&"inner"),
        "expected anchor on 'outer' without 'inner', got {:?}",
        names
    );
}

#[test]
fn ts_nested_arrow_in_method_anchors_on_method() {
    // `const inner = () => { ... }` declared inside a class method body
    // is a local helper. Changes inside it must anchor on the method,
    // not on `inner`. Promotion of `variable_declarator` only applies
    // at the top level (module, class body), never inside a function
    // body.
    let original = "\
export class S {
  doWork(): number {
    const inner = (): number => {
      const x = 1;
      return x;
    };
    return inner();
  }
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork") && !names.contains(&"inner"),
        "expected anchor on 'doWork' without 'inner', got {:?}",
        names
    );
}

#[test]
fn change_in_method_inside_class_anchors_on_method_not_class() {
    // The hunk for a change inside a method must cover the method only.
    // It must NOT climb to the enclosing class and pull in unrelated
    // sibling methods (constructors, other methods) before the change.
    let original = "\
export class S {
  constructor() {}

  alpha() {
    return 1;
  }

  beta() {
    return 2;
  }

  target() {
    const x = 1;
    return x;
  }
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // The hunk must NOT include sibling methods.
    let body: String = hunks[0]
        .lines
        .iter()
        .map(|l| l.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("alpha()") && !body.contains("beta()"),
        "hunk leaked sibling methods (alpha/beta), full body:\n{}",
        body
    );
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"target"),
        "breadcrumb should anchor on 'target', got {:?}",
        names
    );
}

#[test]
fn one_line_change_in_method_shows_full_method() {
    // A one-line change inside a method should expand to cover the
    // whole method body. The reviewer wants to read the function in
    // context, not just three lines around the change.
    let original = "\
fn target() -> i32 {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    a + b + c + d + e + f + g + h
}
";
    let updated = original.replace("let d = 4", "let d = 40");
    let diff = Diff::compute(original, &updated, "test.rs");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // The function is 11 lines. The hunk must include all of them.
    let total = hunks[0].lines.len();
    assert!(
        total >= 11,
        "hunk should include the full method body, got {} lines",
        total
    );
}

#[test]
fn change_at_method_level_decorator_anchors_on_method_not_class() {
    // Method-level decorators (e.g. @EventPattern, @Cron) live in the
    // tree as siblings of the decorated method, not as children. A
    // query at the decorator line previously walked up directly to
    // the class. The hunk for a change on a decorator line must
    // anchor on the method it decorates, not the enclosing class.
    let original = "\
export class S {
  constructor() {}

  alpha() {
    return 1;
  }

  beta() {
    return 2;
  }

  @EventPattern(EVENT.created.pattern)
  async target(event: OldType): Promise<void> {
    return;
  }
}
";
    let updated = original.replace(
        "@EventPattern(EVENT.created.pattern)",
        "@HandleEvent(EVENT.created)",
    );
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1, "a single decorator change must be one hunk");
    let body: String = hunks[0]
        .lines
        .iter()
        .map(|l| l.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("alpha()") && !body.contains("beta()"),
        "hunk leaked sibling methods, full body:\n{}",
        body
    );
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"target"),
        "breadcrumb should anchor on 'target', got {:?}",
        names
    );
}

#[test]
fn change_at_class_level_decorator_anchors_on_class() {
    // Class-level decorators (e.g. @Injectable, @Module) are siblings
    // of `class_declaration` under `export_statement` (or the program
    // root). A change on the decorator line must anchor on the
    // decorated class, not climb past it.
    let original = "\
@Injectable()
export class S {
  doWork(): number {
    return 1;
  }
}
";
    let updated = original.replace("@Injectable()", "@Service()");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1, "a class decorator change must be one hunk");
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"S"),
        "breadcrumb should anchor on the class 'S', got {:?}",
        names
    );
}

#[test]
fn change_in_huge_method_uses_structure_context() {
    // When the innermost method exceeds MAX_SCOPE_LINES, the algorithm
    // must NOT climb to the enclosing class. It uses 100-line context
    // clamped to the method boundaries.
    let mut src = String::from("export class S {\n  huge() {\n");
    for i in 1..=210 {
        src.push_str(&format!("    const a{} = {};\n", i, i));
    }
    src.push_str("  }\n  sibling() { return 0; }\n}\n");
    let original = src.clone();
    let updated = original.replace("const a100 = 100", "const a100 = 1000");
    let diff = Diff::compute(&original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    // Hunk must NOT include the sibling method or the class header.
    let body: String = hunks[0]
        .lines
        .iter()
        .map(|l| l.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("sibling()") && !body.contains("export class S"),
        "hunk leaked sibling/class header for huge method:\n{}",
        body
    );
    // 100-line budget: ~100 before + 1 rm + 1 add + ~100 after ≈ 202 lines.
    // Must not include the full method (212 lines) or the class.
    let total = hunks[0].lines.len();
    assert!(
        (100..=210).contains(&total),
        "huge method should use ~100-line context, got {} lines",
        total
    );
}

#[test]
fn ts_decorated_static_arrow_field_appears_in_breadcrumb() {
    // Decorators and `static` modifiers should not prevent promotion.
    let original = "\
export class S {
  @decorate
  static doStatic = async (): Promise<number> => {
    const x = 1;
    return x;
  };
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doStatic"),
        "breadcrumb should include 'doStatic', got {:?}",
        names
    );
}

#[test]
fn ts_let_arrow_is_promoted() {
    // `let f = () => {...}` produces the same
    // `lexical_declaration > variable_declarator > arrow_function`
    // chain as `const`. Promotion should fire identically.
    let original = "\
export let compute = (): number => {
  const x = 1;
  return x;
};
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "compute.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"compute"),
        "breadcrumb should include 'compute', got {:?}",
        names
    );
}

#[test]
fn ts_top_level_const_function_expression_is_promoted() {
    // The wrapper's value can be `function_expression` instead of
    // `arrow_function`. Both belong to `function_body_kinds`, so
    // both should trigger promotion of the surrounding
    // variable_declarator.
    let original = "\
export const compute = function (): number {
  const x = 1;
  return x;
};
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "compute.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"compute"),
        "breadcrumb should include 'compute', got {:?}",
        names
    );
}

#[test]
fn ts_typed_top_level_arrow_appears_in_breadcrumb() {
    // A top-level const arrow with explicit type annotation should
    // promote like an untyped one.
    let original = "\
export const compute: () => number = () => {
  const x = 1;
  return x;
};
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "compute.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"compute"),
        "breadcrumb should include 'compute', got {:?}",
        names
    );
}

#[test]
fn ts_getter_rename_anchors_on_method() {
    // A getter is just a method_definition; renaming it should produce
    // one hunk anchored on the method.
    let original = "\
export class S {
  get oldName(): number {
    return 1;
  }
}
";
    let updated = original.replace("oldName", "newName");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.iter().any(|n| *n == "oldName" || *n == "newName"),
        "breadcrumb should include the getter name, got {:?}",
        names
    );
}

#[test]
fn ts_constructor_body_edit_anchors_on_constructor() {
    // Constructors have name 'constructor' as a method_definition.
    let original = "\
export class S {
  constructor(private readonly dep: Dep) {
    const x = 1;
    this.dep = dep;
  }
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"constructor"),
        "breadcrumb should include 'constructor', got {:?}",
        names
    );
}

#[test]
fn ts_typed_class_field_with_non_function_value_not_promoted() {
    // `static readonly fancy: number = 42;` must not become a structure;
    // edits inside neighbouring code anchor on the surrounding scope.
    let original = "\
export class S {
  static readonly fancy: number = 42;
  doWork(): number {
    return this.fancy + 1;
  }
}
";
    let updated = original.replace("this.fancy + 1", "this.fancy + 2");
    let diff = Diff::compute(original, &updated, "s.ts");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork") && !names.contains(&"fancy"),
        "expected anchor on doWork without 'fancy', got {:?}",
        names
    );
}

#[test]
fn js_top_level_const_arrow_appears_in_breadcrumb() {
    // JS variant of the TS top-level const arrow test. The
    // `lexical_declaration > variable_declarator > arrow_function`
    // shape is the same as in TS, but exercises the JS language
    // configuration.
    let original = "\
const compute = () => {
  const x = 1;
  return x;
};
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "compute.js");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"compute"),
        "JS breadcrumb should include 'compute', got {:?}",
        names
    );
}

#[test]
fn js_class_arrow_field_appears_in_breadcrumb() {
    // JS uses `field_definition` (no public_ prefix) and the name field
    // is named `property` instead of `name`.
    let original = "\
class S {
  doWork = async () => {
    const x = 1;
    return x;
  };
}
";
    let updated = original.replace("const x = 1", "const x = 10");
    let diff = Diff::compute(original, &updated, "s.js");
    let hunks = diff.hunks();
    assert_eq!(hunks.len(), 1);
    let names: Vec<&str> = hunks[0].ancestors.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"doWork"),
        "JS breadcrumb should include 'doWork', got {:?}",
        names
    );
}

#[test]
fn rename_with_signature_growing_to_multiple_lines() {
    // Rename where the new signature spans more lines than the old.
    // The diff alignment must still classify this as the same slot.
    let original = "\
fn other() -> i32 { 0 }

fn target(a: i32) -> i32 {
    a + 1
}
";
    let updated = "\
fn other() -> i32 { 0 }

fn target_renamed(
    a: i32,
    b: i32,
) -> i32 {
    a + b
}
";
    let diff = Diff::compute(original, updated, "test.rs");
    let hunks = diff.hunks();
    let rename_hunks = hunks
        .iter()
        .filter(|h| {
            h.ancestors
                .iter()
                .any(|a| a.name == "target" || a.name == "target_renamed")
        })
        .count();
    assert_eq!(
        rename_hunks,
        1,
        "multi-line signature change of one function must be one hunk; got hunks {:?}",
        hunks
            .iter()
            .map(|h| h.ancestors.iter().map(|a| &a.name).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn delete_inside_decorator_produces_hunk() {
    // Bug: deleting lines inside a class decorator (e.g. @Module({...}))
    // produces no hunk. `skip_decorators` in enclosing_scopes jumps from
    // the decorator content to the class_declaration, whose line range
    // doesn't cover the deleted lines. The context range anchors on the
    // class (far past the delete) and the hunk builder drops the result
    // as context-only.
    let original = "\
@Decorator({
  items: [
    SomeModule.forRootAsync({
      inject: [SomeService],
      imports: [],
      inject: [SomeService],
      useFactory: (cfg) => {
        return cfg;
      },
    }),
  ],
})
export class MyModule {}
";
    let updated = "\
@Decorator({
  items: [
    SomeModule.forRootAsync({
      inject: [SomeService],
      useFactory: (cfg) => {
        return cfg;
      },
    }),
  ],
})
export class MyModule {}
";

    let diff = Diff::compute(original, updated, "test.ts");
    assert!(!diff.text().is_empty(), "engine should detect a diff");
    let hunks = diff.hunks();
    assert_eq!(
        hunks.len(),
        1,
        "delete inside decorator should produce 1 hunk, got 0"
    );
    let removed: Vec<&str> = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .map(|l| l.content.as_str())
        .collect();
    assert!(
        removed.iter().any(|l| l.contains("imports: []")),
        "removed lines should include 'imports: []', got {:?}",
        removed
    );
}
