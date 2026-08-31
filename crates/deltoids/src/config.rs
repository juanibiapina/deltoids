//! Configuration loading for deltoids.
//!
//! Loads theme settings from `$XDG_CONFIG_HOME/deltoids/config.toml`.
//! Also provides syntax highlighting asset loading.

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::env;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(not(target_arch = "wasm32"))]
use bat::assets::HighlightingAssets;
#[cfg(not(target_arch = "wasm32"))]
use serde::Deserialize;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
#[cfg(not(target_arch = "wasm32"))]
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
    /// Foreground for added status (staged column, `+N` counts, added files).
    pub status_added: (u8, u8, u8),
    /// Foreground for deleted status (worktree column, `-N` counts, deletes).
    pub status_deleted: (u8, u8, u8),
    /// Foreground for modified status (piped-diff fallback letter).
    pub status_modified: (u8, u8, u8),
    /// Foreground for partial-stage tint (name/dir yellow) and renames.
    pub status_partial: (u8, u8, u8),
    /// Foreground for copied status (piped-diff fallback letter).
    pub status_copied: (u8, u8, u8),
    /// Foreground for type-change status (piped-diff fallback letter).
    pub status_typechange: (u8, u8, u8),
    /// Registry name of the syntax theme used to color diff bodies and
    /// breadcrumbs. Resolved by [`Theme::load`] (explicit `[theme]
    /// syntax_theme` key → `BAT_THEME` → mode default) and passed to
    /// [`crate::theme_by_name`] by the renderers. `default`/`for_mode` set the
    /// mode default so a plain `Theme::default()` stays self-consistent.
    pub syntax_theme_name: String,
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
            status_added: (158, 206, 106),       // #9ece6a
            status_deleted: (247, 118, 142),     // #f7768e
            status_modified: (247, 118, 142),    // #f7768e
            status_partial: (224, 175, 104),     // #e0af68
            status_copied: (125, 207, 255),      // #7dcfff
            status_typechange: (187, 154, 247),  // #bb9af7
            syntax_theme_name: default_syntax_theme_name(ColorMode::Dark).to_string(),
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
                // Status foregrounds match the dark palette; they read on
                // cream as well as on the dark default.
                status_added: (158, 206, 106),
                status_deleted: (247, 118, 142),
                status_modified: (247, 118, 142),
                status_partial: (224, 175, 104),
                status_copied: (125, 207, 255),
                status_typechange: (187, 154, 247),
                syntax_theme_name: default_syntax_theme_name(ColorMode::Light).to_string(),
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load() -> Self {
        let (explicit, overlay) = read_user_theme_config().unwrap_or_default();
        let mode = load_color_mode(explicit);
        let mut theme = resolve_theme(mode, &overlay);
        theme.syntax_theme_name = resolve_selected_syntax_theme_name(&overlay, mode);
        theme
    }
}

/// Resolve the selected syntax-theme name for [`Theme::load`].
///
/// Precedence: an explicit `[theme] syntax_theme` key, then `BAT_THEME`, then
/// the light/dark mode default. Names are validated against the registry
/// ([`theme_names`]); an unknown name falls through to the next source so a
/// typo silently degrades to the mode default rather than a broken theme.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_selected_syntax_theme_name(overlay: &ThemeConfig, mode: ColorMode) -> String {
    let bat = env::var("BAT_THEME").ok();
    resolve_syntax_theme_from(overlay.syntax_theme.as_deref(), bat.as_deref(), mode)
}

/// Pure precedence for the selected syntax-theme name: explicit `[theme]
/// syntax_theme` key, then `BAT_THEME`, then the mode default. Each candidate
/// is accepted only when the registry knows it, so a typo degrades to the next
/// source. Split out from env/config IO so precedence is unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_syntax_theme_from(
    explicit: Option<&str>,
    bat_theme: Option<&str>,
    mode: ColorMode,
) -> String {
    let known = |name: &str| theme_names().contains(&name);

    if let Some(name) = explicit
        && known(name)
    {
        return name.to_string();
    }
    if let Some(name) = bat_theme
        && known(name)
    {
        return name.to_string();
    }
    default_syntax_theme_name(mode).to_string()
}

/// Read the user's `[theme]` config, returning `(explicit_mode, overlay)`.
///
/// Returns `None` if the file is missing, unreadable, or fails to parse so
/// the caller can fall back to defaults silently.
#[cfg(not(target_arch = "wasm32"))]
fn read_user_theme_config() -> Option<(Option<ColorMode>, ThemeConfig)> {
    let path = config_file_path()?;
    let contents = fs::read_to_string(&path).ok()?;
    parse_theme_config(&contents)
}

#[cfg(not(target_arch = "wasm32"))]
fn load_color_mode(explicit: Option<ColorMode>) -> ColorMode {
    if let Some(mode) = explicit {
        mode
    } else {
        resolve_color_mode(None, detect_color_mode())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_color_mode(explicit: Option<ColorMode>, detected: Option<ColorMode>) -> ColorMode {
    explicit.or(detected).unwrap_or(ColorMode::Dark)
}

#[cfg(not(target_arch = "wasm32"))]
fn detect_color_mode() -> Option<ColorMode> {
    theme_mode(QueryOptions::default()).ok().map(|m| match m {
        ThemeMode::Light => ColorMode::Light,
        ThemeMode::Dark => ColorMode::Dark,
    })
}

// Delta's defaults for syntax themes. Available on both targets so
// `Theme::default`/`Theme::for_mode` can set the mode default name.
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load() -> Self {
        let syntax_set = SYNTAX_SET.get_or_init(bundled_syntax_set);

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

    /// Load syntax assets on wasm from two-face's embedded syntect dumps (the
    /// same syntaxes/themes bat ships), avoiding the filesystem and oniguruma.
    #[cfg(target_arch = "wasm32")]
    pub fn load() -> Self {
        let syntax_set = SYNTAX_SET.get_or_init(bundled_syntax_set);

        let syntax_theme = SYNTAX_THEME.get_or_init(|| {
            two_face::theme::extra()
                .get(two_face::theme::EmbeddedThemeName::MonokaiExtended)
                .clone()
        });

        Self {
            syntax_set,
            syntax_theme,
        }
    }

    pub fn syntax_for_name(&self, name: Option<&str>) -> &'static SyntaxReference {
        name.and_then(|name| self.syntax_set.find_syntax_by_name(name))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }
}

// ---------------------------------------------------------------------------
// Syntax-theme registry
//
// One name-keyed registry of syntect themes, populated identically in spirit
// on native (from bat's `HighlightingAssets`) and wasm (from two-face's
// embedded dumps), plus the vendored Tokyo Night converted to a dump by
// `build.rs`. Every renderer resolves its theme through [`theme_by_name`] as
// an explicit input; `None` / unknown fall back to today's global default so
// behaviour is byte-identical until a caller passes a real name.
// ---------------------------------------------------------------------------

/// Registry key for the vendored Tokyo Night theme. Matches the `name` field
/// inside `assets/themes/tokyonight.tmTheme`.
pub const TOKYO_NIGHT_THEME_NAME: &str = "TokyoNight";

/// Compressed syntect dump of the vendored Tokyo Night theme, produced by
/// `build.rs` from `assets/themes/tokyonight.tmTheme`. Loaded with
/// `from_binary` (dump-load) on both targets.
static TOKYO_NIGHT_DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tokyonight.themedump"));

fn tokyo_night_theme() -> SyntectTheme {
    syntect::dumps::from_binary(TOKYO_NIGHT_DUMP)
}

struct ThemeRegistry {
    themes: HashMap<&'static str, SyntectTheme>,
    names: Vec<&'static str>,
}

static THEME_REGISTRY: OnceLock<ThemeRegistry> = OnceLock::new();

fn theme_registry() -> &'static ThemeRegistry {
    THEME_REGISTRY.get_or_init(build_theme_registry)
}

fn build_theme_registry() -> ThemeRegistry {
    let mut themes: HashMap<&'static str, SyntectTheme> = HashMap::new();

    #[cfg(not(target_arch = "wasm32"))]
    {
        let assets = load_highlighting_assets();
        for name in assets.themes() {
            let key: &'static str = Box::leak(name.to_string().into_boxed_str());
            themes.insert(key, assets.get_theme(name).clone());
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        let set = two_face::theme::extra();
        for name in two_face::theme::EmbeddedLazyThemeSet::theme_names() {
            // `as_name()` is already `&'static str`.
            themes.insert(name.as_name(), set.get(*name).clone());
        }
    }

    // Vendored Tokyo Night on both targets. `or_insert_with` so a same-named
    // upstream theme (if any) does not get clobbered silently.
    themes
        .entry(TOKYO_NIGHT_THEME_NAME)
        .or_insert_with(tokyo_night_theme);

    let mut names: Vec<&'static str> = themes.keys().copied().collect();
    names.sort_unstable();
    ThemeRegistry { themes, names }
}

/// Today's global default syntax theme: the value the renderers used before
/// the theme became an explicit parameter. `theme_by_name(None)` returns this,
/// so unspecified/unknown themes are byte-identical to prior behaviour.
fn default_syntax_theme() -> &'static SyntectTheme {
    SyntaxAssets::load().syntax_theme
}

/// Resolve a syntax theme by registry name. `None` or an unknown name falls
/// back to [`default_syntax_theme`] (the mode/`BAT_THEME` default on native,
/// Monokai Extended on wasm), so callers that do not select a theme keep the
/// prior colors exactly.
pub fn theme_by_name(name: Option<&str>) -> &'static SyntectTheme {
    if let Some(name) = name
        && let Some(theme) = theme_registry().themes.get(name)
    {
        return theme;
    }
    default_syntax_theme()
}

/// All registry theme names, sorted. Backs the frontends' theme pickers.
pub fn theme_names() -> &'static [&'static str] {
    &theme_registry().names
}

/// The registry's `&'static` spelling of `name`, or `""` when unknown.
///
/// Lets a `Copy` cache key (e.g. the TUI's `CacheEpoch`) carry the theme
/// identity without owning a `String`: two epochs compare equal iff they
/// name the same registry theme. Unknown names collapse to `""`, which is
/// harmless because selection is always constrained to registry names.
pub fn theme_name_key(name: &str) -> &'static str {
    theme_names()
        .iter()
        .copied()
        .find(|candidate| *candidate == name)
        .unwrap_or("")
}

/// The bundled syntax set used for both highlighting and stable language
/// detection. Native builds read bat's embedded assets; wasm builds use
/// two-face's embedded syntect dumps.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bundled_syntax_set() -> SyntaxSet {
    load_highlighting_assets()
        .get_syntax_set()
        .expect("bundled syntax assets should load")
        .clone()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn bundled_syntax_set() -> SyntaxSet {
    two_face::syntax::extra_newlines()
}

#[cfg(not(target_arch = "wasm32"))]
fn load_highlighting_assets() -> HighlightingAssets {
    let cache_dir = bat_cache_dir().map(|d| d.join("bat"));
    cache_dir
        .and_then(|dir| HighlightingAssets::from_cache(&dir).ok())
        .unwrap_or_else(HighlightingAssets::from_binary)
}

/// Get the cache directory following bat/delta conventions.
/// On macOS, follows XDG spec (XDG_CACHE_HOME or ~/.cache) rather than native paths.
/// On other platforms, uses the native cache directory.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn resolve_theme(mode: ColorMode, overlay: &ThemeConfig) -> Theme {
    let base = Theme::for_mode(mode);
    apply_overlay(base, overlay)
}

#[cfg(not(target_arch = "wasm32"))]
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
        status_added: parse_hex_color(&overlay.status_added).unwrap_or(base.status_added),
        status_deleted: parse_hex_color(&overlay.status_deleted).unwrap_or(base.status_deleted),
        status_modified: parse_hex_color(&overlay.status_modified).unwrap_or(base.status_modified),
        status_partial: parse_hex_color(&overlay.status_partial).unwrap_or(base.status_partial),
        status_copied: parse_hex_color(&overlay.status_copied).unwrap_or(base.status_copied),
        status_typechange: parse_hex_color(&overlay.status_typechange)
            .unwrap_or(base.status_typechange),
        // Carried through unchanged; [`Theme::load`] overwrites it with the
        // fully-resolved name (explicit key → BAT_THEME → mode default).
        syntax_theme_name: base.syntax_theme_name,
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    theme: Option<ThemeConfig>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    /// `"light"`, `"dark"`, or `"auto"` (default).
    mode: Option<String>,
    /// Registry name of the syntax theme (e.g. `"TokyoNight"`, `"GitHub"`).
    /// Takes precedence over `BAT_THEME` and the mode default.
    syntax_theme: Option<String>,
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
    status_added: Option<String>,
    status_deleted: Option<String>,
    status_modified: Option<String>,
    status_partial: Option<String>,
    status_copied: Option<String>,
    status_typechange: Option<String>,
}

/// Parse a deltoids `config.toml` body into `(explicit_mode, overlay)`.
///
/// `explicit_mode` is `Some` only when the user wrote `mode = "light"` or
/// `mode = "dark"`; `mode = "auto"`, missing, or absent `[theme]` all yield
/// `None` so the caller can fall back to detection.
///
/// Returns `None` on TOML parse failure or unknown mode strings, letting the
/// caller decide whether to ignore the file or surface an error.
#[cfg(not(target_arch = "wasm32"))]
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

/// Path to the deltoids `config.toml`, following XDG resolution
/// (`$XDG_CONFIG_HOME/deltoids/config.toml`, falling back to
/// `~/.config/deltoids/config.toml`). `None` when no config home can be
/// resolved. Exposed so the CLI reads the same file for its own sections
/// (e.g. `[[commands]]`) without duplicating the resolution logic.
#[cfg(not(target_arch = "wasm32"))]
pub fn config_file_path() -> Option<PathBuf> {
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;

    Some(config_home.join("deltoids").join("config.toml"))
}

/// Parse a hex color string like "#2a4556" into an RGB tuple.
#[cfg(not(target_arch = "wasm32"))]
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
            status_added: Some("#445566".into()),
            ..Default::default()
        };
        let theme = resolve_theme(ColorMode::Light, &overlay);
        // Override wins for the specified fields.
        assert_eq!(theme.diff_added_bg, (0x11, 0x22, 0x33));
        assert_eq!(theme.status_added, (0x44, 0x55, 0x66));
        // Other fields retain the light palette.
        let light = Theme::for_mode(ColorMode::Light);
        assert_eq!(theme.diff_deleted_bg, light.diff_deleted_bg);
        assert_eq!(theme.separator, light.separator);
        assert_eq!(theme.status_deleted, light.status_deleted);
        assert_eq!(theme.status_partial, light.status_partial);
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
        assert_eq!(theme.status_added, (158, 206, 106));
        assert_eq!(theme.status_deleted, (247, 118, 142));
        assert_eq!(theme.status_partial, (224, 175, 104));
        assert_eq!(theme.status_copied, (125, 207, 255));
        assert_eq!(theme.status_typechange, (187, 154, 247));
    }

    #[test]
    fn rgb_to_ansi_fg_produces_correct_sequence() {
        assert_eq!(rgb_to_ansi_fg(122, 162, 247), "\x1b[38;2;122;162;247m");
    }

    #[test]
    fn rgb_to_ansi_bg_produces_correct_sequence() {
        assert_eq!(rgb_to_ansi_bg(32, 48, 59), "\x1b[48;2;32;48;59m");
    }

    #[test]
    fn registry_lists_vendored_and_bundled_themes() {
        let names = theme_names();
        assert!(
            names.contains(&TOKYO_NIGHT_THEME_NAME),
            "registry must include the vendored Tokyo Night, got {names:?}"
        );
        // A couple of bat-bundled themes we rely on for defaults.
        assert!(names.contains(&"GitHub"));
        assert!(names.contains(&"Monokai Extended"));
        // Sorted and unique.
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        assert_eq!(names, sorted.as_slice());
    }

    #[test]
    fn tokyo_night_loads_from_vendored_dump() {
        let theme = theme_by_name(Some(TOKYO_NIGHT_THEME_NAME));
        assert_eq!(theme.name.as_deref(), Some("TokyoNight"));
        // Background from assets/themes/tokyonight.tmTheme: #1a1b26.
        let bg = theme
            .settings
            .background
            .expect("tokyonight has a background");
        assert_eq!((bg.r, bg.g, bg.b), (0x1a, 0x1b, 0x26));
    }

    #[test]
    fn distinct_names_resolve_to_distinct_themes() {
        let tokyo = theme_by_name(Some(TOKYO_NIGHT_THEME_NAME));
        let github = theme_by_name(Some("GitHub"));
        assert_ne!(
            tokyo.settings.background, github.settings.background,
            "Tokyo Night and GitHub should differ in background"
        );
    }

    #[test]
    fn syntax_theme_precedence_explicit_key_wins() {
        // A valid explicit key beats BAT_THEME and the mode default.
        let name = resolve_syntax_theme_from(
            Some(TOKYO_NIGHT_THEME_NAME),
            Some("GitHub"),
            ColorMode::Dark,
        );
        assert_eq!(name, TOKYO_NIGHT_THEME_NAME);
    }

    #[test]
    fn syntax_theme_precedence_bat_theme_beats_mode_default() {
        // No explicit key: a valid BAT_THEME wins over the mode default.
        let name = resolve_syntax_theme_from(None, Some("GitHub"), ColorMode::Dark);
        assert_eq!(name, "GitHub");
    }

    #[test]
    fn syntax_theme_precedence_falls_back_to_mode_default() {
        // Neither source valid: dark → Monokai Extended, light → GitHub.
        assert_eq!(
            resolve_syntax_theme_from(None, None, ColorMode::Dark),
            "Monokai Extended"
        );
        assert_eq!(
            resolve_syntax_theme_from(None, None, ColorMode::Light),
            "GitHub"
        );
    }

    #[test]
    fn syntax_theme_precedence_unknown_names_degrade() {
        // Unknown explicit key and unknown BAT_THEME both fall through.
        let name = resolve_syntax_theme_from(Some("Nope"), Some("AlsoNope"), ColorMode::Light);
        assert_eq!(name, "GitHub");
    }

    #[test]
    fn parse_theme_config_extracts_syntax_theme_key() {
        let toml = r#"
            [theme]
            syntax_theme = "TokyoNight"
        "#;
        let (_mode, overlay) = parse_theme_config(toml).expect("valid TOML");
        assert_eq!(overlay.syntax_theme.as_deref(), Some("TokyoNight"));
    }

    #[test]
    fn unknown_and_none_fall_back_to_default() {
        // Byte-identical to the pre-registry global default, so unspecified /
        // unknown themes never drift the TUI/pager/serve output.
        let default = default_syntax_theme();
        assert_eq!(theme_by_name(None).settings, default.settings);
        assert_eq!(
            theme_by_name(Some("no-such-theme-xyz")).settings,
            default.settings
        );
    }
}
