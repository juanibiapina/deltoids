//! Configuration loading for deltoids.
//!
//! Loads theme settings from `$XDG_CONFIG_HOME/deltoids/config.toml`.
//! Also provides syntax highlighting asset loading.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use bat::assets::HighlightingAssets;
use serde::Deserialize;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

/// Theme colors used by deltoids rendering.
///
/// All colors are stored as RGB tuples `(r, g, b)`.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Background for added diff lines.
    pub diff_added_bg: (u8, u8, u8),
    /// Background for emphasized (intraline) added regions.
    pub diff_added_emph_bg: (u8, u8, u8),
    /// Background for deleted diff lines.
    pub diff_deleted_bg: (u8, u8, u8),
    /// Background for emphasized (intraline) deleted regions.
    pub diff_deleted_emph_bg: (u8, u8, u8),
    /// Separator line color (file headers).
    pub separator: (u8, u8, u8),
    /// Border color (breadcrumb boxes, inactive panes).
    pub border: (u8, u8, u8),
    /// Active border color (focused panes).
    pub border_active: (u8, u8, u8),
    /// Line number color.
    pub line_number: (u8, u8, u8),
    /// Muted text color (metadata, help).
    pub muted: (u8, u8, u8),
    /// Background color for selected items.
    pub selection_bg: (u8, u8, u8),
}

impl Default for Theme {
    fn default() -> Self {
        // Tokyo Night inspired RGB values.
        Self {
            diff_added_bg: (32, 48, 59),         // #20303b
            diff_added_emph_bg: (44, 90, 102),   // #2c5a66
            diff_deleted_bg: (55, 34, 44),       // #37222c
            diff_deleted_emph_bg: (113, 49, 55), // #713137
            separator: (122, 162, 247),          // #7aa2f7
            border: (122, 162, 247),             // #7aa2f7
            border_active: (255, 150, 108),      // #ff966c
            line_number: (122, 162, 247),        // #7aa2f7
            muted: (86, 95, 137),                // #565f89
            selection_bg: (45, 63, 118),         // #2d3f76
        }
    }
}

impl Theme {
    /// Load theme from config file, falling back to defaults per-field.
    pub fn load() -> Self {
        let Some(path) = config_file_path() else {
            return Self::default();
        };

        let Ok(contents) = fs::read_to_string(&path) else {
            return Self::default();
        };

        let Ok(config) = toml::from_str::<ConfigFile>(&contents) else {
            return Self::default();
        };

        let defaults = Self::default();
        let theme = config.theme.unwrap_or_default();

        Self {
            diff_added_bg: parse_hex_color(&theme.diff_added_bg).unwrap_or(defaults.diff_added_bg),
            diff_added_emph_bg: parse_hex_color(&theme.diff_added_emph_bg)
                .unwrap_or(defaults.diff_added_emph_bg),
            diff_deleted_bg: parse_hex_color(&theme.diff_deleted_bg)
                .unwrap_or(defaults.diff_deleted_bg),
            diff_deleted_emph_bg: parse_hex_color(&theme.diff_deleted_emph_bg)
                .unwrap_or(defaults.diff_deleted_emph_bg),
            separator: parse_hex_color(&theme.separator).unwrap_or(defaults.separator),
            border: parse_hex_color(&theme.border).unwrap_or(defaults.border),
            border_active: parse_hex_color(&theme.border_active).unwrap_or(defaults.border_active),
            line_number: parse_hex_color(&theme.line_number).unwrap_or(defaults.line_number),
            muted: parse_hex_color(&theme.muted).unwrap_or(defaults.muted),
            selection_bg: parse_hex_color(&theme.selection_bg).unwrap_or(defaults.selection_bg),
        }
    }
}

// Delta's defaults for syntax themes.
const DEFAULT_DARK_THEME: &str = "Monokai Extended";
const DEFAULT_LIGHT_THEME: &str = "GitHub";

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static SYNTAX_THEME: OnceLock<SyntectTheme> = OnceLock::new();

/// Loaded syntax highlighting assets.
pub struct SyntaxAssets {
    pub syntax_set: &'static SyntaxSet,
    pub syntax_theme: &'static SyntectTheme,
}

impl SyntaxAssets {
    /// Load syntax assets from bat cache or binary fallback.
    ///
    /// Uses `BAT_THEME` if set, otherwise detects dark/light mode and uses
    /// appropriate defaults (Monokai Extended for dark, GitHub for light).
    pub fn load() -> Self {
        let syntax_set = SYNTAX_SET.get_or_init(|| {
            load_highlighting_assets()
                .get_syntax_set()
                .expect("syntax assets should load")
                .clone()
        });

        let syntax_theme = SYNTAX_THEME.get_or_init(|| {
            let assets = load_highlighting_assets();
            let theme_name = resolve_syntax_theme_name(&assets);
            assets.get_theme(&theme_name).clone()
        });

        Self {
            syntax_set,
            syntax_theme,
        }
    }
}

fn load_highlighting_assets() -> HighlightingAssets {
    let cache_dir = bat_cache_dir().map(|d| d.join("bat"));
    cache_dir
        .and_then(|dir| HighlightingAssets::from_cache(&dir).ok())
        .unwrap_or_else(HighlightingAssets::from_binary)
}

/// Get the cache directory following bat/delta conventions.
/// On macOS, follows XDG spec (XDG_CACHE_HOME or ~/.cache) rather than native paths.
/// On other platforms, uses the native cache directory.
fn bat_cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
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

fn resolve_syntax_theme_name(assets: &HighlightingAssets) -> String {
    // 1. Check BAT_THEME
    if let Ok(theme) = env::var("BAT_THEME") {
        if assets.themes().any(|t| t == theme) {
            return theme;
        }
    }

    // 2. Use default based on light/dark
    let is_light = detect_is_light_mode();
    if is_light {
        DEFAULT_LIGHT_THEME
    } else {
        DEFAULT_DARK_THEME
    }
    .to_string()
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    theme: Option<ThemeConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    diff_added_bg: Option<String>,
    diff_added_emph_bg: Option<String>,
    diff_deleted_bg: Option<String>,
    diff_deleted_emph_bg: Option<String>,
    separator: Option<String>,
    border: Option<String>,
    border_active: Option<String>,
    line_number: Option<String>,
    muted: Option<String>,
    selection_bg: Option<String>,
}

fn config_file_path() -> Option<PathBuf> {
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;

    Some(config_home.join("deltoids").join("config.toml"))
}

/// Parse a hex color string like "#2a4556" into an RGB tuple.
fn parse_hex_color(s: &Option<String>) -> Option<(u8, u8, u8)> {
    let s = s.as_ref()?;
    let s = s.strip_prefix('#').unwrap_or(s);

    if s.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;

    Some((r, g, b))
}

/// Convert RGB tuple to ANSI foreground escape sequence.
pub fn rgb_to_ansi_fg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{};{};{}m", r, g, b)
}

/// Convert RGB tuple to ANSI background escape sequence.
pub fn rgb_to_ansi_bg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[48;2;{};{};{}m", r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color_parses_with_hash() {
        assert_eq!(parse_hex_color(&Some("#2a4556".into())), Some((42, 69, 86)));
    }

    #[test]
    fn parse_hex_color_parses_without_hash() {
        assert_eq!(parse_hex_color(&Some("2a4556".into())), Some((42, 69, 86)));
    }

    #[test]
    fn parse_hex_color_returns_none_for_invalid() {
        assert_eq!(parse_hex_color(&Some("invalid".into())), None);
        assert_eq!(parse_hex_color(&Some("#12".into())), None);
        assert_eq!(parse_hex_color(&None), None);
    }

    #[test]
    fn default_theme_has_expected_values() {
        let theme = Theme::default();
        assert_eq!(theme.diff_added_bg, (32, 48, 59));
        assert_eq!(theme.diff_deleted_bg, (55, 34, 44));
        assert_eq!(theme.separator, (122, 162, 247));
        assert_eq!(theme.border, (122, 162, 247));
    }

    #[test]
    fn rgb_to_ansi_fg_produces_correct_sequence() {
        assert_eq!(rgb_to_ansi_fg(122, 162, 247), "\x1b[38;2;122;162;247m");
    }

    #[test]
    fn rgb_to_ansi_bg_produces_correct_sequence() {
        assert_eq!(rgb_to_ansi_bg(32, 48, 59), "\x1b[48;2;32;48;59m");
    }
}
