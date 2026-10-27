#![cfg(not(feature = "lang-kotlin"))]

use deltoids::Diff;

#[test]
fn a_language_without_its_grammar_diffs_without_scope_context() {
    let original = "class Greeter {\n    fun greet() {\n        println(\"hi\")\n    }\n}\n";
    let updated = original.replace("\"hi\"", "\"hello\"");

    let diff = Diff::compute(original, &updated, "Greeter.kt");

    assert!(!diff.hunks().is_empty());
    assert!(diff.hunks().iter().all(|hunk| hunk.ancestors.is_empty()));
}
