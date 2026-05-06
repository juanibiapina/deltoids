//! Rendering of structural changes for plain-text and TUI surfaces.
//!
//! Stage five of the structural-diff layer. Lives in `deltoids` (not in
//! a CLI crate) so every consumer — `deltoids` filter, `rv`, `edit-tui`,
//! anything else — can draw the same summary without re-implementing
//! the formatting rules.
//!
//! Two output formats:
//!
//! - [`format_summary`] — multi-line plain text, one change per line:
//!
//!   ```text
//!   3 structural changes in src/foo.rs:
//!     + Added function `parse` (public)
//!     ~ Modified method `Foo::bar`
//!     - Removed function `legacy_helper`
//!   ```
//!
//! - [`format_summary_compact`] — single line with totals only,
//!   useful for status bars / footers:
//!
//!   ```text
//!   +1 ~1 -1
//!   ```
//!
//! Both formats are stable: tests pin the strings.

use super::classify::ChangeKind;
use super::diff::StructuralDiff;

/// Multi-line summary suitable for terminal output. Empty diff → empty
/// string.
pub fn format_summary(diff: &StructuralDiff) -> String {
    format_summary_with(diff, &SummaryOptions::default())
}

/// Single-line totals (`+N ~N -N`). Empty diff → empty string.
pub fn format_summary_compact(diff: &StructuralDiff) -> String {
    let totals = totals(diff);
    if totals.is_zero() {
        return String::new();
    }
    format!("+{} ~{} -{}", totals.added, totals.modified, totals.removed)
}

/// Tuneable formatter — picks an indent prefix and an optional title.
pub fn format_summary_with(diff: &StructuralDiff, opts: &SummaryOptions) -> String {
    if diff.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if opts.title {
        let totals = totals(diff);
        let n = diff.len();
        let plural = if n == 1 { "change" } else { "changes" };
        out.push_str(&format!(
            "{} structural {plural} (+{} ~{} -{}):\n",
            n, totals.added, totals.modified, totals.removed,
        ));
    }
    let it: Box<dyn Iterator<Item = _>> = if opts.public_only {
        Box::new(diff.public_changes())
    } else if opts.signatures_only {
        Box::new(diff.signature_changes())
    } else {
        Box::new(diff.changes().iter())
    };
    for change in it {
        let bullet = bullet_for(change.kind);
        out.push_str(opts.indent);
        out.push(bullet);
        out.push(' ');
        if opts.show_signatures {
            out.push_str(&format_signature_line(change));
        } else {
            out.push_str(&change.description);
        }
        out.push('\n');
    }
    out
}

/// Render a `StructuralChange` as a one-line signature listing:
/// `<sig>` for added/removed,
/// `<old_sig> → <new_sig>` for signature-changed/renamed,
/// `<sig>` (no arrow) for body-only modifications.
/// Falls back to the description when the change has no signature on
/// either side (Added with no recorded signature, etc.).
fn format_signature_line(change: &super::classify::StructuralChange) -> String {
    let before = change.before.as_ref().map(|s| s.signature.as_str());
    let after = change.after.as_ref().map(|s| s.signature.as_str());
    match (before, after) {
        (Some(b), Some(a)) if b == a => a.to_string(),
        (Some(b), Some(a)) => format!("{b}  →  {a}"),
        (None, Some(a)) | (Some(a), None) => a.to_string(),
        (None, None) => change.description.clone(),
    }
}

/// Counts of changes by polarity. Modified/Renamed/SignatureChanged/
/// VisibilityChanged/BodyChanged all roll up under `modified` so the
/// short summary stays readable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
}

impl Totals {
    pub fn total(self) -> usize {
        self.added + self.removed + self.modified
    }

    pub fn is_zero(self) -> bool {
        self.total() == 0
    }
}

/// Aggregate change totals.
pub fn totals(diff: &StructuralDiff) -> Totals {
    let mut t = Totals::default();
    for change in diff.changes() {
        match change.kind {
            ChangeKind::Added => t.added += 1,
            ChangeKind::Removed => t.removed += 1,
            ChangeKind::Modified
            | ChangeKind::Renamed
            | ChangeKind::BodyChanged
            | ChangeKind::SignatureChanged
            | ChangeKind::VisibilityChanged => t.modified += 1,
        }
    }
    t
}

/// Options for [`format_summary_with`].
#[derive(Debug, Clone)]
pub struct SummaryOptions<'a> {
    pub indent: &'a str,
    pub title: bool,
    pub public_only: bool,
    pub signatures_only: bool,
    /// When true, render the raw declaration signature instead of the
    /// human-readable description (e.g. "pub fn parse(path: &str) -> i32"
    /// instead of "Modified function `parse`"). Useful for the
    /// signatures-only view in the TUI / CLI.
    pub show_signatures: bool,
}

impl Default for SummaryOptions<'_> {
    fn default() -> Self {
        Self {
            indent: "  ",
            title: true,
            public_only: false,
            signatures_only: false,
            show_signatures: false,
        }
    }
}

fn bullet_for(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => '+',
        ChangeKind::Removed => '-',
        ChangeKind::Renamed => '→',
        ChangeKind::SignatureChanged
        | ChangeKind::VisibilityChanged
        | ChangeKind::Modified
        | ChangeKind::BodyChanged => '~',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str, path: &str) -> StructuralDiff {
        StructuralDiff::compute(old, new, path)
    }

    #[test]
    fn empty_diff_renders_to_empty_string() {
        let d = diff("fn x() {}\n", "fn x() {}\n", "a.rs");
        assert_eq!(format_summary(&d), "");
        assert_eq!(format_summary_compact(&d), "");
    }

    #[test]
    fn summary_lists_added_and_removed() {
        let old = "fn one() {}\nfn two() {}\n";
        let new = "fn one() {}\nfn three() {}\n";
        let d = diff(old, new, "a.rs");
        let s = format_summary(&d);
        assert!(s.contains("Added function `three`"), "{s}");
        assert!(s.contains("Removed function `two`"), "{s}");
    }

    #[test]
    fn compact_summary_shows_totals() {
        let old = "fn a() {}\nfn b() {}\n";
        let new = "fn a() {}\nfn c() {}\nfn d() {}\n";
        let d = diff(old, new, "a.rs");
        // a -> a (unchanged), b removed, c added, d added → +2 ~0 -1
        assert_eq!(format_summary_compact(&d), "+2 ~0 -1");
    }

    #[test]
    fn public_only_option_filters() {
        let old = "fn priv_a() {}\npub fn pub_a() {}\n";
        let new = "fn priv_a() { let x = 1; }\npub fn pub_a() { let y = 2; }\n";
        let d = diff(old, new, "a.rs");
        let opts = SummaryOptions {
            public_only: true,
            ..SummaryOptions::default()
        };
        let s = format_summary_with(&d, &opts);
        assert!(s.contains("pub_a"), "{s}");
        assert!(!s.contains("priv_a"), "{s}");
    }

    #[test]
    fn title_includes_total_count() {
        let new = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let d = diff("", new, "a.rs");
        let s = format_summary(&d);
        assert!(s.starts_with("3 structural changes"), "{s}");
    }

    #[test]
    fn show_signatures_renders_signature_change_with_arrow() {
        let old = "pub fn add(a: i32) -> i32 {\n    a\n}\n";
        let new = "pub fn add(a: i32, b: i32) -> i32 {\n    a\n}\n";
        let d = diff(old, new, "a.rs");
        let opts = SummaryOptions {
            show_signatures: true,
            title: false,
            ..SummaryOptions::default()
        };
        let s = format_summary_with(&d, &opts);
        assert!(s.contains("pub fn add(a: i32) -> i32"), "got:\n{s}");
        assert!(s.contains("pub fn add(a: i32, b: i32) -> i32"), "got:\n{s}");
        assert!(s.contains("→"), "got:\n{s}");
    }

    #[test]
    fn show_signatures_renders_added_signature_only() {
        let old = "";
        let new = "pub fn brand_new() -> &'static str { \"hi\" }\n";
        let d = diff(old, new, "a.rs");
        let opts = SummaryOptions {
            show_signatures: true,
            title: false,
            ..SummaryOptions::default()
        };
        let s = format_summary_with(&d, &opts);
        assert!(
            s.contains("pub fn brand_new() -> &'static str"),
            "got:\n{s}"
        );
        assert!(!s.contains("→"), "got:\n{s}");
    }
}
