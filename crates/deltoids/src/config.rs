//! Configuration loading for deltoids.
//!
//! Loads theme settings from `$XDG_CONFIG_HOME/deltoids/config.toml`.

use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

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
