mod change_layout;
pub mod config;
#[cfg(feature = "blob-resolve")]
pub mod content;
pub mod definitions;
mod engine;
#[cfg(feature = "blob-resolve")]
pub mod git;
mod highlight;
mod hunk_header;
mod intraline;
mod language;
mod line_map;
pub mod parse;
pub mod render;
#[cfg(feature = "html")]
pub mod render_html;
#[cfg(feature = "ratatui")]
pub mod render_tui;
pub mod reverse;
mod scope;
mod symbols;
pub mod symlink;
mod syntax;

pub use change_layout::{ChangeLayout, arrange_change};
pub use config::{
    ColorMode, SyntaxAssets, TOKYO_NIGHT_THEME_NAME, Theme, theme_by_name, theme_name_key,
    theme_names,
};
pub use definitions::{
    Aspects, DefinitionChange, DefinitionDiff, DefinitionKind, Place, Status, TopLevelChange,
};
pub use intraline::{EmphKind, EmphSection, LineEmphasis, compute_subhunk_emphasis};
pub use language::Language;
pub use line_map::LineMap;
pub use scope::{Diff, DiffLine, Hunk, HunkRun, LineKind, ScopeNode};
pub use symbols::Symbols;
pub use symlink::SymlinkView;
