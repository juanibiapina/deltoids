pub mod config;
mod intraline;
pub mod parse;
pub mod render;
pub mod reverse;
mod scope;
pub mod syntax;

pub use config::Theme;
pub use intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
pub use scope::{Diff, DiffLine, Hunk, LineKind, ScopeNode};
