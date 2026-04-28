pub mod config;
mod engine;
mod intraline;
pub mod parse;
pub mod render;
pub mod reverse;
mod scope;
pub mod syntax;

pub use config::{SyntaxAssets, Theme};
pub use engine::{DiffOp, Snapshot};
pub use intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
pub use scope::{Diff, DiffLine, Hunk, HunkRun, LineKind, ScopeNode};
