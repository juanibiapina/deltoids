pub mod scope;
pub mod syntax;

pub use scope::{DiffLine, Hunk, LineKind, ScopeNode, enrich_diff};

use similar::TextDiff;

/// Generate a plain unified diff without scope injection.
pub fn raw_unified_diff(original: &str, updated: &str) -> String {
    let text_diff = TextDiff::from_lines(original, updated);
    let mut diff = text_diff.unified_diff();
    diff.context_radius(3).header("original", "modified");
    diff.to_string()
}
