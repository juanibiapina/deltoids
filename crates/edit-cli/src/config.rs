//! Configuration loading for edit-cli.
//!
//! Loads UI theme settings from `$XDG_CONFIG_HOME/deltoids/config.toml`.

use std::env;
use std::fs;
use std::path::PathBuf;

use ratatui::style::Color;
use serde::Deserialize;

/// UI theme colors used by the TUI.
#[derive(Debug, Clone)]
pub struct UiTheme {
    /// Background for added diff lines.
    pub diff_added_bg: Color,
    /// Background for emphasized (intraline) added regions.
    pub diff_added_emph_bg: Color,
    /// Background for deleted diff lines.
    pub diff_deleted_bg: Color,
    /// Background for emphasized (intraline) deleted regions.
    pub diff_deleted_emph_bg: Color,
    /// Border color for inactive panes.
    pub border: Color,
    /// Border color for the active pane.
    pub border_active: Color,
    /// Background color for selected items.
    pub selection_bg: Color,
    /// Dimmed text color (metadata, help).
    pub dim: Color,
}

impl Default for UiTheme {
    fn default() -> Self {
        // Diff colors use Tokyo Night inspired RGB values.
        // UI chrome colors use ANSI colors for terminal compatibility.
        Self {
            diff_added_bg: Color::Rgb(42, 69, 86),
            diff_added_emph_bg: Color::Rgb(48, 95, 111),
            diff_deleted_bg: Color::Rgb(75, 42, 61),
            diff_deleted_emph_bg: Color::Rgb(107, 46, 67),
            border: Color::Blue,
            border_active: Color::Yellow,
            selection_bg: Color::DarkGray,
            dim: Color::Gray,
        }
    }
}

impl UiTheme {
    /// Load theme from config file, falling back to defaults.
    pub fn load() -> Self {
        let config_path = config_file_path();
        let Some(path) = config_path else {
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
            diff_added_bg: parse_color(&theme.diff_added_bg).unwrap_or(defaults.diff_added_bg),
            diff_added_emph_bg: parse_color(&theme.diff_added_emph_bg)
                .unwrap_or(defaults.diff_added_emph_bg),
            diff_deleted_bg: parse_color(&theme.diff_deleted_bg)
                .unwrap_or(defaults.diff_deleted_bg),
            diff_deleted_emph_bg: parse_color(&theme.diff_deleted_emph_bg)
                .unwrap_or(defaults.diff_deleted_emph_bg),
            border: parse_color(&theme.border).unwrap_or(defaults.border),
            border_active: parse_color(&theme.border_active).unwrap_or(defaults.border_active),
            selection_bg: parse_color(&theme.selection_bg).unwrap_or(defaults.selection_bg),
            dim: parse_color(&theme.dim).unwrap_or(defaults.dim),
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
    border: Option<String>,
    border_active: Option<String>,
    selection_bg: Option<String>,
    dim: Option<String>,
}

fn config_file_path() -> Option<PathBuf> {
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;

    Some(config_home.join("deltoids").join("config.toml"))
}

/// Parse a hex color string like "#2a4556" into a Color.
fn parse_color(s: &Option<String>) -> Option<Color> {
    let s = s.as_ref()?;
    let s = s.strip_prefix('#').unwrap_or(s);

    if s.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;

    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_parses_hex_with_hash() {
        assert_eq!(
            parse_color(&Some("#2a4556".into())),
            Some(Color::Rgb(42, 69, 86))
        );
    }

    #[test]
    fn parse_color_parses_hex_without_hash() {
        assert_eq!(
            parse_color(&Some("2a4556".into())),
            Some(Color::Rgb(42, 69, 86))
        );
    }

    #[test]
    fn parse_color_returns_none_for_invalid() {
        assert_eq!(parse_color(&Some("invalid".into())), None);
        assert_eq!(parse_color(&Some("#12".into())), None);
        assert_eq!(parse_color(&None), None);
    }

    #[test]
    fn default_theme_uses_rgb_for_diff_and_ansi_for_chrome() {
        let theme = UiTheme::default();
        // Diff colors use RGB
        assert_eq!(theme.diff_added_bg, Color::Rgb(42, 69, 86));
        assert_eq!(theme.diff_deleted_bg, Color::Rgb(75, 42, 61));
        // UI chrome uses ANSI
        assert_eq!(theme.border, Color::Blue);
        assert_eq!(theme.border_active, Color::Yellow);
        assert_eq!(theme.selection_bg, Color::DarkGray);
        assert_eq!(theme.dim, Color::Gray);
    }
}
