pub mod config;
#[cfg(feature = "blob-resolve")]
pub mod content;
mod engine;
#[cfg(feature = "blob-resolve")]
pub mod git;
mod highlight;
mod hunk_header;
mod intraline;
mod language;
pub mod parse;
pub mod render;
#[cfg(feature = "ratatui")]
pub mod render_tui;
pub mod reverse;
mod scope;
pub mod syntax;

pub use config::{ColorMode, SyntaxAssets, Theme};
pub use engine::{DiffOp, Snapshot};
pub use intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
pub use language::Language;
pub use scope::{Diff, DiffLine, Hunk, HunkRun, LineKind, ScopeNode};
