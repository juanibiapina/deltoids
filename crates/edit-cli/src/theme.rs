//! Theme resolution for syntax highlighting.
//!
//! Loads syntax themes via the `bat` crate (same as delta).

use bat::assets::HighlightingAssets;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

use crate::config::UiTheme;

// Delta's defaults
const DEFAULT_DARK_THEME: &str = "Monokai Extended";
const DEFAULT_LIGHT_THEME: &str = "GitHub";

/// Resolved theme with syntax highlighting and UI colors.
pub struct ResolvedTheme {
    pub syntax_theme: Theme,
    pub syntax_set: SyntaxSet,
    pub ui: UiTheme,
}

impl ResolvedTheme {
    /// Resolve theme from environment and config.
    ///
    /// Uses `BAT_THEME` if set, otherwise detects dark/light mode and uses
    /// appropriate defaults (Monokai Extended for dark, GitHub for light).
    /// UI colors come from config file or defaults.
    pub fn resolve() -> Self {
        let assets = load_assets();
        let is_light = detect_is_light_mode();
        let theme_name = resolve_theme_name(is_light, &assets);
        let syntax_theme = assets.get_theme(&theme_name).clone();
        let syntax_set = assets
            .get_syntax_set()
            .expect("bat assets should include syntax set")
            .clone();

        Self {
            syntax_theme,
            syntax_set,
            ui: UiTheme::load(),
        }
    }
}

fn load_assets() -> HighlightingAssets {
    let cache_dir = bat_cache_dir().map(|d| d.join("bat"));
    cache_dir
        .and_then(|dir| HighlightingAssets::from_cache(&dir).ok())
        .unwrap_or_else(HighlightingAssets::from_binary)
}

/// Get the cache directory following bat/delta conventions.
/// On macOS, follows XDG spec (XDG_CACHE_HOME or ~/.cache) rather than native paths.
/// On other platforms, uses the native cache directory.
fn bat_cache_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|d| d.join(".cache")))
    }

    #[cfg(not(target_os = "macos"))]
    {
        dirs::cache_dir()
    }
}

fn detect_is_light_mode() -> bool {
    theme_mode(QueryOptions::default())
        .map(|m| matches!(m, ThemeMode::Light))
        .unwrap_or(false)
}

fn resolve_theme_name(is_light: bool, assets: &HighlightingAssets) -> String {
    // 1. Check BAT_THEME
    if let Ok(theme) = std::env::var("BAT_THEME") {
        if assets.themes().any(|t| t == theme) {
            return theme;
        }
    }

    // 2. Use default based on light/dark
    if is_light {
        DEFAULT_LIGHT_THEME
    } else {
        DEFAULT_DARK_THEME
    }
    .to_string()
}
