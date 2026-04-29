//! Theme resolution for the TUI.
//!
//! Combines syntax highlighting assets from deltoids with UI theme colors.

use ratatui::style::Color;

use deltoids::{SyntaxAssets, Theme};

/// Resolved theme: syntax assets (set + theme + lookup) plus UI colors.
pub struct ResolvedTheme {
    pub syntax_assets: SyntaxAssets,
    pub ui: Theme,
}

impl ResolvedTheme {
    /// Resolve theme from environment and config.
    pub fn resolve() -> Self {
        Self {
            syntax_assets: SyntaxAssets::load(),
            ui: Theme::load(),
        }
    }
}

/// Convert RGB tuple to ratatui Color.
pub fn to_color(rgb: (u8, u8, u8)) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}
