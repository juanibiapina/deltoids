//! Theme resolution for the TUI.
//!
//! Combines syntax highlighting assets from deltoids with UI theme colors.

use ratatui::style::Color;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

pub use deltoids::{SyntaxAssets, Theme};

/// Resolved theme with syntax highlighting and UI colors.
pub struct ResolvedTheme {
    pub syntax_theme: &'static SyntectTheme,
    pub syntax_set: &'static SyntaxSet,
    pub ui: Theme,
}

impl ResolvedTheme {
    /// Resolve theme from environment and config.
    pub fn resolve() -> Self {
        let assets = SyntaxAssets::load();

        Self {
            syntax_theme: assets.syntax_theme,
            syntax_set: assets.syntax_set,
            ui: Theme::load(),
        }
    }
}

/// Convert RGB tuple to ratatui Color.
pub fn to_color(rgb: (u8, u8, u8)) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}
