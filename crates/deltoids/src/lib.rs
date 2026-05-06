pub mod config;
#[cfg(feature = "blob-resolve")]
pub mod content;
mod engine;
#[cfg(feature = "blob-resolve")]
pub mod git;
mod intraline;
mod language;
pub mod parse;
pub mod render;
#[cfg(feature = "ratatui")]
pub mod render_tui;
pub mod reverse;
mod scope;
pub mod structural;
pub mod syntax;

pub use config::{ColorMode, SyntaxAssets, Theme};
pub use engine::{DiffOp, Snapshot};
pub use intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
pub use language::Language;
pub use scope::{Diff, DiffLine, Hunk, HunkRun, LineKind, ScopeNode};
pub use structural::{LineSpan, StructuralDiff, Symbol, SymbolKind, SymbolPath, Visibility};
