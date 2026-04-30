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
use syntect::parsing::{SyntaxReference, SyntaxSet};
use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

/// Whether the surrounding terminal is light or dark.
///
/// Determines which built-in palette [`Theme::for_mode`] returns and is the
/// signal we use to pick a default syntax theme too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Light,
    Dark,
}

/// Theme colors used by deltoids rendering.
///
/// All colors are stored as RGB tuples `(r, g, b)`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Built-in palette for the given [`ColorMode`].
    ///
    /// `Dark` returns the same RGBs as [`Theme::default`]. `Light` returns a
    /// pastel-on-cream palette inspired by delta's defaults.
    pub fn for_mode(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Dark => Self::default(),
            ColorMode::Light => Self {
                diff_added_bg: (0xd0, 0xff, 0xd0),
                diff_added_emph_bg: (0xa0, 0xef, 0xa0),
                diff_deleted_bg: (0xff, 0xe0, 0xe0),
                diff_deleted_emph_bg: (0xff, 0xc0, 0xc0),
                // Chrome accents stay the saturated Tokyo Night blue/orange;
                // they read on cream as well as on the dark default.
                separator: (122, 162, 247),
                border: (122, 162, 247),
                border_active: (255, 150, 108),
                line_number: (122, 162, 247),
                muted: (113, 121, 158),
                selection_bg: (212, 222, 252),
            },
        }
    }

    /// Load theme by combining config file, terminal detection, and built-in palettes.
    ///
    /// Resolution order for the palette:
    ///   1. `[theme] mode = "light"|"dark"` in `$XDG_CONFIG_HOME/deltoids/config.toml`.
    ///   2. `mode = "auto"` (default): query the terminal via
    ///      [`terminal_colorsaurus`].
    ///   3. Fall back to [`ColorMode::Dark`].
    ///
    /// Per-field hex overrides in the same `[theme]` section then patch the
    /// chosen palette.
    pub fn load() -> Self {
        let (explicit, overlay) = read_user_theme_config().unwrap_or_default();
        resolve_theme(load_color_mode(explicit), &overlay)
    }
}

/// Read the user's `[theme]` config, returning `(explicit_mode, overlay)`.
///
/// Returns `None` if the file is missing, unreadable, or fails to parse so
/// the caller can fall back to defaults silently.
fn read_user_theme_config() -> Option<(Option<ColorMode>, ThemeConfig)> {
    let path = config_file_path()?;
    let contents = fs::read_to_string(&path).ok()?;
    parse_theme_config(&contents)
}

fn load_color_mode(explicit: Option<ColorMode>) -> ColorMode {
    if let Some(mode) = explicit {
        mode
    } else {
        resolve_color_mode(None, detect_color_mode())
    }
}

fn resolve_color_mode(explicit: Option<ColorMode>, detected: Option<ColorMode>) -> ColorMode {
    explicit.or(detected).unwrap_or(ColorMode::Dark)
}

fn detect_color_mode() -> Option<ColorMode> {
    theme_mode(QueryOptions::default()).ok().map(|m| match m {
        ThemeMode::Light => ColorMode::Light,
        ThemeMode::Dark => ColorMode::Dark,
    })
}

// Delta's defaults for syntax themes.
const DEFAULT_DARK_SYNTAX_THEME: &str = "Monokai Extended";
const DEFAULT_LIGHT_SYNTAX_THEME: &str = "GitHub";

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
    /// Uses `BAT_THEME` if set. Otherwise uses `[theme] mode` (or terminal
    /// detection when mode is `auto`) to choose appropriate defaults: Monokai
    /// Extended for dark, GitHub for light.
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

    /// Look up the syntax to use for an already-detected deltoids language.
    ///
    /// Returns the bundled plain-text syntax when `language` is `None` or
    /// unsupported. Detection is the caller's job (`Diff::compute` resolves
    /// it through `Language::detect`); rendering should never re-detect from
    /// a single line.
    pub fn syntax_for(&self, language: Option<crate::Language>) -> &'static SyntaxReference {
        language
            .and_then(|language| {
                self.syntax_set
                    .find_syntax_by_token(language.syntax_token())
            })
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
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

fn resolve_syntax_theme_name(assets: &HighlightingAssets) -> String {
    // 1. Check BAT_THEME
    if let Ok(theme) = env::var("BAT_THEME")
        && assets.themes().any(|t| t == theme)
    {
        return theme;
    }

    // 2. Use the resolved light/dark mode to pick a default syntax theme.
    // This makes `[theme] mode = "light"` behave like delta's `light = true`:
    // it affects both diff backgrounds and the syntax theme fallback.
    let (explicit, _) = read_user_theme_config().unwrap_or_default();
    default_syntax_theme_name(load_color_mode(explicit)).to_string()
}

fn default_syntax_theme_name(mode: ColorMode) -> &'static str {
    match mode {
        ColorMode::Light => DEFAULT_LIGHT_SYNTAX_THEME,
        ColorMode::Dark => DEFAULT_DARK_SYNTAX_THEME,
    }
}

/// Resolve a [`Theme`] from a resolved color mode and user overrides.
///
/// Pure: takes whatever mode the caller has already resolved and patches
/// per-field hex overrides on top of the chosen built-in palette. The impure
/// orchestration (file IO, terminal probing) lives in [`Theme::load`].
fn resolve_theme(mode: ColorMode, overlay: &ThemeConfig) -> Theme {
    let base = Theme::for_mode(mode);
    apply_overlay(base, overlay)
}

fn apply_overlay(base: Theme, overlay: &ThemeConfig) -> Theme {
    Theme {
        diff_added_bg: parse_hex_color(&overlay.diff_added_bg).unwrap_or(base.diff_added_bg),
        diff_added_emph_bg: parse_hex_color(&overlay.diff_added_emph_bg)
            .unwrap_or(base.diff_added_emph_bg),
        diff_deleted_bg: parse_hex_color(&overlay.diff_deleted_bg).unwrap_or(base.diff_deleted_bg),
        diff_deleted_emph_bg: parse_hex_color(&overlay.diff_deleted_emph_bg)
            .unwrap_or(base.diff_deleted_emph_bg),
        separator: parse_hex_color(&overlay.separator).unwrap_or(base.separator),
        border: parse_hex_color(&overlay.border).unwrap_or(base.border),
        border_active: parse_hex_color(&overlay.border_active).unwrap_or(base.border_active),
        line_number: parse_hex_color(&overlay.line_number).unwrap_or(base.line_number),
        muted: parse_hex_color(&overlay.muted).unwrap_or(base.muted),
        selection_bg: parse_hex_color(&overlay.selection_bg).unwrap_or(base.selection_bg),
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    theme: Option<ThemeConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    /// `"light"`, `"dark"`, or `"auto"` (default).
    mode: Option<String>,
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

/// Parse a deltoids `config.toml` body into `(explicit_mode, overlay)`.
///
/// `explicit_mode` is `Some` only when the user wrote `mode = "light"` or
/// `mode = "dark"`; `mode = "auto"`, missing, or absent `[theme]` all yield
/// `None` so the caller can fall back to detection.
///
/// Returns `None` on TOML parse failure or unknown mode strings, letting the
/// caller decide whether to ignore the file or surface an error.
fn parse_theme_config(text: &str) -> Option<(Option<ColorMode>, ThemeConfig)> {
    let config: ConfigFile = toml::from_str(text).ok()?;
    let overlay = config.theme.unwrap_or_default();
    let mode = match overlay.mode.as_deref() {
        None | Some("auto") => None,
        Some("light") => Some(ColorMode::Light),
        Some("dark") => Some(ColorMode::Dark),
        Some(_) => return None,
    };
    Some((mode, overlay))
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
    fn for_mode_dark_matches_default() {
        assert_eq!(Theme::for_mode(ColorMode::Dark), Theme::default());
    }

    #[test]
    fn resolve_color_mode_uses_explicit_mode_when_set() {
        assert_eq!(
            resolve_color_mode(Some(ColorMode::Light), None),
            ColorMode::Light
        );
    }

    #[test]
    fn resolve_color_mode_falls_back_to_dark_when_nothing_known() {
        assert_eq!(resolve_color_mode(None, None), ColorMode::Dark);
    }

    #[test]
    fn resolve_color_mode_uses_detected_mode_when_no_explicit() {
        assert_eq!(
            resolve_color_mode(None, Some(ColorMode::Light)),
            ColorMode::Light
        );
    }

    #[test]
    fn resolve_color_mode_explicit_beats_detected() {
        assert_eq!(
            resolve_color_mode(Some(ColorMode::Dark), Some(ColorMode::Light)),
            ColorMode::Dark
        );
    }

    #[test]
    fn resolve_theme_applies_field_overrides_on_top_of_palette() {
        let overlay = ThemeConfig {
            diff_added_bg: Some("#112233".into()),
            ..Default::default()
        };
        let theme = resolve_theme(ColorMode::Light, &overlay);
        // Override wins for the specified field.
        assert_eq!(theme.diff_added_bg, (0x11, 0x22, 0x33));
        // Other fields retain the light palette.
        let light = Theme::for_mode(ColorMode::Light);
        assert_eq!(theme.diff_deleted_bg, light.diff_deleted_bg);
        assert_eq!(theme.separator, light.separator);
    }

    #[test]
    fn parse_theme_config_extracts_light_mode() {
        let toml = r#"
            [theme]
            mode = "light"
        "#;
        let (mode, _overlay) = parse_theme_config(toml).expect("valid TOML");
        assert_eq!(mode, Some(ColorMode::Light));
    }

    #[test]
    fn parse_theme_config_extracts_dark_mode() {
        let toml = r#"
            [theme]
            mode = "dark"
        "#;
        let (mode, _overlay) = parse_theme_config(toml).expect("valid TOML");
        assert_eq!(mode, Some(ColorMode::Dark));
    }

    #[test]
    fn parse_theme_config_treats_auto_as_no_explicit_mode() {
        let toml = r#"
            [theme]
            mode = "auto"
        "#;
        let (mode, _overlay) = parse_theme_config(toml).expect("valid TOML");
        assert_eq!(mode, None);
    }

    #[test]
    fn parse_theme_config_returns_none_when_mode_absent() {
        let toml = r##"
            [theme]
            diff_added_bg = "#112233"
        "##;
        let (mode, overlay) = parse_theme_config(toml).expect("valid TOML");
        assert_eq!(mode, None);
        assert_eq!(overlay.diff_added_bg.as_deref(), Some("#112233"));
    }

    #[test]
    fn parse_theme_config_rejects_unknown_mode() {
        let toml = r#"
            [theme]
            mode = "sepia"
        "#;
        assert!(parse_theme_config(toml).is_none());
    }

    #[test]
    fn light_mode_uses_light_syntax_theme_fallback() {
        assert_eq!(default_syntax_theme_name(ColorMode::Light), "GitHub");
    }

    #[test]
    fn dark_mode_uses_dark_syntax_theme_fallback() {
        assert_eq!(
            default_syntax_theme_name(ColorMode::Dark),
            "Monokai Extended"
        );
    }

    #[test]
    fn for_mode_light_uses_light_diff_backgrounds() {
        let theme = Theme::for_mode(ColorMode::Light);
        assert_eq!(theme.diff_added_bg, (0xd0, 0xff, 0xd0));
        assert_eq!(theme.diff_added_emph_bg, (0xa0, 0xef, 0xa0));
        assert_eq!(theme.diff_deleted_bg, (0xff, 0xe0, 0xe0));
        assert_eq!(theme.diff_deleted_emph_bg, (0xff, 0xc0, 0xc0));
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
