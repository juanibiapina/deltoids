mod intraline;
mod scope;
pub mod parse;
pub mod render;
pub mod reverse;
pub mod syntax;

pub use intraline::{compute_subhunk_emphasis, EmphKind, EmphSection, LineEmphasis};
pub use scope::{Diff, DiffLine, Hunk, LineKind, ScopeNode};
