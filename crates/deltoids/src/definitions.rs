//! What a change did to a file's definitions: which functions, types,
//! constants, sections, and keys were added, removed, modified, renamed,
//! or moved.
//!
//! [`DefinitionDiff::compute`] lists the definitions of both versions of
//! a file (see `syntax::outline`), pairs them up (`matching`), and says
//! what changed in each pair (`classify`). Definitions are the scopes the
//! breadcrumbs name, plus each language's constants, type aliases, and
//! keys; all of it comes from the language's
//! [`TreeSitterConfig`](crate::language::TreeSitterConfig).

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::Language;
use crate::engine::{DiffOp, Snapshot};
use crate::syntax::ParsedFile;

mod classify;
mod matching;

/// What a definition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DefinitionKind {
    Function,
    Method,
    Constructor,
    Class,
    Struct,
    Enum,
    Union,
    Interface,
    Trait,
    Impl,
    Module,
    Namespace,
    Object,
    /// A type declaration whose kind its own syntax does not say (Go).
    Type,
    TypeAlias,
    Constant,
    Variable,
    Macro,
    /// A labeled call such as `it("…")`, `describe("…")`, `t.Run("…")`.
    Test,
    Section,
    Table,
    Rule,
    Block,
    Statement,
    Key,
}

impl DefinitionKind {
    /// Lowercase name, as the test format and the TUI print it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Constructor => "constructor",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Union => "union",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Module => "module",
            Self::Namespace => "namespace",
            Self::Object => "object",
            Self::Type => "type",
            Self::TypeAlias => "type alias",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Macro => "macro",
            Self::Test => "test",
            Self::Section => "section",
            Self::Table => "table",
            Self::Rule => "rule",
            Self::Block => "block",
            Self::Statement => "statement",
            Self::Key => "key",
        }
    }

    /// Kinds in one family may pair up across versions: a function that
    /// became a method, a struct that became a class.
    pub(crate) fn family(self) -> Family {
        match self {
            Self::Function | Self::Method | Self::Constructor | Self::Test | Self::Macro => {
                Family::Callable
            }
            Self::Class
            | Self::Struct
            | Self::Enum
            | Self::Union
            | Self::Interface
            | Self::Trait
            | Self::Object
            | Self::Type
            | Self::TypeAlias => Family::Type,
            Self::Impl
            | Self::Module
            | Self::Namespace
            | Self::Section
            | Self::Table
            | Self::Rule
            | Self::Block
            | Self::Statement => Family::Container,
            Self::Constant | Self::Variable | Self::Key => Family::Data,
        }
    }

    /// Types whose functions are methods.
    pub(crate) fn holds_methods(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Struct
                | Self::Enum
                | Self::Union
                | Self::Interface
                | Self::Trait
                | Self::Impl
                | Self::Object
                | Self::Type
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    Callable,
    Type,
    Container,
    Data,
}

/// Definition changes between two versions of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionDiff {
    /// Changed definitions, outermost first. Present definitions come in
    /// new-file order; a removed one sits right after the definition that
    /// preceded it in the old file.
    pub changes: Vec<DefinitionChange>,
    /// Code outside every definition, when it changed.
    pub top_level: Option<TopLevelChange>,
    /// Either version has syntax errors.
    pub has_errors: bool,
}

impl DefinitionDiff {
    /// The definition changes from `original` to `updated`, the language
    /// detected from `path`. `None` when the language has no tree-sitter
    /// support.
    pub fn compute(original: &str, updated: &str, path: &str) -> Option<DefinitionDiff> {
        let language =
            Language::detect(path, updated).or_else(|| Language::detect(path, original))?;
        let snapshot = Snapshot::compute(original, updated);
        Self::of(language, original, updated, snapshot.ops())
    }

    /// The definition changes for a line diff already computed as `ops`.
    pub(crate) fn of(
        language: Language,
        original: &str,
        updated: &str,
        ops: &[DiffOp],
    ) -> Option<DefinitionDiff> {
        let old = ParsedFile::parse_as(language, original)?.outline();
        let new = ParsedFile::parse_as(language, updated)?.outline();
        let changed =
            classify::ChangedRows::of(ops, original.lines().count(), updated.lines().count());
        let pairing = matching::pair(&old, &new, &changed.kept);
        Some(classify::diff(
            &old, &new, &pairing, &changed, original, updated,
        ))
    }

    /// Nothing changed in any definition or at the top level.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.top_level.is_none()
    }
}

/// One changed definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionChange {
    pub kind: DefinitionKind,
    /// Its name in the new file, or in the old one when removed.
    pub name: String,
    pub status: Status,
    /// What changed in its own lines; nested definitions are not part of
    /// them.
    pub aspects: Aspects,
    /// Where it is in the old file; `None` when added.
    pub old: Option<Place>,
    /// Where it is in the new file; `None` when removed.
    pub new: Option<Place>,
    /// Changed lines among its own lines: (added, removed).
    pub lines: (usize, usize),
    /// Changed definitions nested inside it.
    pub children: Vec<DefinitionChange>,
    /// Its text, or its counterpart's, has a syntax error.
    pub uncertain: bool,
}

/// How a definition changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Added,
    Removed,
    Modified,
    Renamed {
        from: String,
    },
    /// Moved without other change of identity: to another parent (`from`
    /// names the old one, outermost first, empty for the top level), or
    /// to another place in the same parent (`from` is `None`).
    Moved {
        from: Option<Vec<String>>,
    },
    /// Itself unchanged; listed because a nested definition changed.
    Container,
}

/// What changed in a definition's own lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aspects {
    /// Its header: everything from its first token to its body.
    pub signature: bool,
    /// The attributes or decorators above it.
    pub attributes: bool,
    pub body: bool,
    /// Comments above it.
    pub docs: bool,
    /// Comments inside it.
    pub comments: bool,
    /// Only whitespace, layout, or line endings changed.
    pub formatting: bool,
    /// Imports changed (top level only).
    pub imports: bool,
}

impl Aspects {
    pub fn is_empty(self) -> bool {
        self == Aspects::default()
    }
}

/// Where a definition sits in one version of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// 1-based lines, the comments and attributes above it included.
    pub lines: RangeInclusive<usize>,
    /// Its header on one line: from its first token to its body, with
    /// runs of whitespace collapsed.
    pub signature: String,
    /// Its first changed line in this version, 1-based.
    pub first_change: Option<usize>,
}

/// A change to the code outside every definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopLevelChange {
    pub aspects: Aspects,
    /// Changed top-level lines: (added, removed).
    pub lines: (usize, usize),
    /// First changed top-level line in the new file, 1-based.
    pub new_first_change: Option<usize>,
    /// First changed top-level line in the old file, 1-based.
    pub old_first_change: Option<usize>,
}

#[cfg(test)]
mod perf {
    use std::time::Instant;

    use super::*;

    /// A file of `count` small definitions; `edit` changes every tenth.
    fn generated(path: &str, count: usize, edit: bool) -> String {
        let value = |i: usize| {
            if edit && i.is_multiple_of(10) {
                i + 1
            } else {
                i
            }
        };
        let each = |i: usize| -> String {
            let k = value(i);
            match path.rsplit('.').next().unwrap_or("") {
                "rs" => {
                    format!("fn f{i}(x: u32) -> u32 {{\n    let y = x + {k};\n    y * 2\n}}\n\n")
                }
                "ts" => format!(
                    "export function f{i}(x: number): number {{\n  const y = x + {k};\n  return y * 2;\n}}\n\n"
                ),
                "js" => {
                    format!("function f{i}(x) {{\n  const y = x + {k};\n  return y * 2;\n}}\n\n")
                }
                "py" => format!("def f{i}(x):\n    y = x + {k}\n    return y * 2\n\n\n"),
                "go" => format!("func F{i}(x int) int {{\n\ty := x + {k}\n\treturn y * 2\n}}\n\n"),
                "java" => format!(
                    "  int f{i}(int x) {{\n    int y = x + {k};\n    return y * 2;\n  }}\n\n"
                ),
                "c" => {
                    format!("int f{i}(int x) {{\n    int y = x + {k};\n    return y * 2;\n}}\n\n")
                }
                "rb" => format!("def f{i}(x)\n  y = x + {k}\n  y * 2\nend\n\n"),
                "md" => format!("## Section {i}\n\nText {k}.\n\n"),
                "yaml" => format!("k{i}:\n  a: {k}\n  b: x\n"),
                "toml" => format!("[t{i}]\na = {k}\nb = \"x\"\n\n"),
                "css" => format!(".c{i} {{ width: {k}px; color: red; }}\n"),
                "json" => format!(
                    "  \"k{i}\": {{\"a\": {k}, \"b\": [1, 2]}}{}\n",
                    if i + 1 == count { "" } else { "," }
                ),
                other => panic!("no generator for {other}"),
            }
        };
        let body: String = (0..count).map(each).collect();
        match path.rsplit('.').next().unwrap_or("") {
            "go" => format!("package p\n\n{body}"),
            "java" => format!("class C {{\n{body}}}\n"),
            "json" => format!("{{\n{body}}}\n"),
            _ => body,
        }
    }

    #[test]
    #[ignore = "release performance probe; run with --release --ignored --nocapture"]
    fn definition_changes_cost_a_fraction_of_parsing() {
        let paths = [
            "a.rs", "a.ts", "a.js", "a.py", "a.go", "A.java", "a.c", "a.rb", "a.md", "a.yaml",
            "a.toml", "a.css", "a.json",
        ];
        let mut over = Vec::new();
        for path in paths {
            let before = generated(path, 5_000, false);
            let after = generated(path, 5_000, true);
            let language = Language::detect(path, &after).expect("supported");
            let start = Instant::now();
            let old = ParsedFile::parse_as(language, &before).expect("parses");
            let new = ParsedFile::parse_as(language, &after).expect("parses");
            let parse = start.elapsed();
            let start = Instant::now();
            let (old, new) = (old.outline(), new.outline());
            let outline = start.elapsed();
            let snapshot = Snapshot::compute(&before, &after);
            let start = Instant::now();
            let changed = classify::ChangedRows::of(
                snapshot.ops(),
                before.lines().count(),
                after.lines().count(),
            );
            let pairing = matching::pair(&old, &new, &changed.kept);
            let diff = classify::diff(&old, &new, &pairing, &changed, &before, &after);
            let pairing_time = start.elapsed();
            let share = (outline + pairing_time).as_secs_f64() / parse.as_secs_f64();
            println!(
                "{path:8} definitions {:5}  changes {:4}  parse {parse:>10.2?}  outline {outline:>10.2?}  pair+classify {pairing_time:>10.2?}  share {:>5.1}%",
                old.definitions.len(),
                diff.changes.len(),
                share * 100.0
            );
            // The tree walk alone costs about a fifth of parsing. Data files
            // here hold three keys a line, every one a definition.
            let budget = if ["json", "yaml", "toml"]
                .iter()
                .any(|ext| path.ends_with(ext))
            {
                1.75
            } else {
                0.75
            };
            if share > budget {
                over.push(format!("{path}: {:.1}% of parsing", share * 100.0));
            }
        }
        assert!(over.is_empty(), "over budget: {over:?}");
    }
}
