//! The shell's settings from the shared `config.toml`: custom key
//! commands and the `[tui]` section.
//!
//! Custom key commands are user-configured keybindings that run a shell
//! command against the currently-selected file. They come from the
//! `[[commands]]` array. Each command binds a single `key` to a shell
//! `command` template. The template's `{{filename}}` token expands to the
//! shell-quoted absolute path of the selected file (see [`expand`]).
//!
//! Two run modes (chosen per command by the `subprocess` flag):
//! - background (default): the shell dispatches the command and keeps
//!   rendering, never touching the terminal (no flicker).
//! - subprocess (`subprocess = true`): the TUI suspends, hands the
//!   terminal to the child (an inline editor), then restores.
//!
//! This module owns loading, parsing, and placeholder expansion; the
//! run paths live in [`super::suspend`] and routing in [`super`].

use std::path::Path;

use serde::Deserialize;

/// One configured custom command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CustomCommand {
    /// The single key that triggers the command.
    pub(super) key: char,
    /// The shell command template (e.g. `"nvim {{filename}}"`).
    pub(super) command: String,
    /// `true` to suspend the TUI and run in the foreground; `false`
    /// (default) to dispatch in the background without touching the
    /// terminal.
    pub(super) subprocess: bool,
    /// Human description for the help popup.
    pub(super) description: String,
}

/// The settings the shell reads from `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShellConfig {
    pub(super) commands: Vec<CustomCommand>,
    /// Whether `Esc` quits once there is nothing left to back out of.
    pub(super) quit_on_esc: bool,
}

/// The CLI's view of `config.toml`: the `[[commands]]` array and the
/// `[tui]` section. Other sections (`[theme]`, `[lsp.<name>]`) are
/// ignored by serde, so this parses the same file the library reads for
/// the theme.
#[derive(Debug, Default, Deserialize)]
struct CliConfig {
    #[serde(default)]
    commands: Vec<RawCommand>,
    #[serde(default)]
    tui: TuiConfig,
}

/// The `[tui]` section.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct TuiConfig {
    quit_on_esc: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { quit_on_esc: true }
    }
}

/// A raw `[[commands]]` table before narrowing `key` to a single char.
#[derive(Debug, Deserialize)]
struct RawCommand {
    key: String,
    command: String,
    #[serde(default)]
    subprocess: bool,
    #[serde(default)]
    description: Option<String>,
}

/// Load the shell's settings from the shared `config.toml`.
///
/// A missing file, unreadable file, or parse error yields the defaults
/// (silent fallback, matching the theme-loading policy).
pub(super) fn load_config() -> ShellConfig {
    let text = deltoids::config::config_file_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    parse_config(&text)
}

/// Parse a `config.toml` body into the shell's settings.
///
/// Pure and unit-testable. TOML parse failure yields the defaults.
/// Commands whose `key` is not exactly one character are dropped.
fn parse_config(text: &str) -> ShellConfig {
    let config = toml::from_str::<CliConfig>(text).unwrap_or_default();
    ShellConfig {
        commands: custom_commands(config.commands),
        quit_on_esc: config.tui.quit_on_esc,
    }
}

/// Narrow raw `[[commands]]` tables to single-key custom commands.
fn custom_commands(raw: Vec<RawCommand>) -> Vec<CustomCommand> {
    raw.into_iter()
        .filter_map(|raw| {
            let mut chars = raw.key.chars();
            let key = chars.next()?;
            if chars.next().is_some() {
                // Multi-char key: drop in v1.
                return None;
            }
            Some(CustomCommand {
                key,
                command: raw.command,
                subprocess: raw.subprocess,
                description: raw.description.unwrap_or_default(),
            })
        })
        .collect()
}

/// Expand a command template against the selected `file`.
///
/// v1 recognises exactly one placeholder, `{{filename}}` (lazygit's
/// spelling), replaced with the shell-quoted absolute path so paths with
/// spaces survive `sh -c`. This is literal substitution, not a template
/// engine: other `{{...}}` tokens are left verbatim.
pub(super) fn expand(template: &str, file: &Path) -> String {
    template.replace("{{filename}}", &shell_quote(&file.to_string_lossy()))
}

/// Single-quote a string for a POSIX shell, escaping embedded quotes.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_commands(text: &str) -> Vec<CustomCommand> {
        parse_config(text).commands
    }

    #[test]
    fn parse_config_reads_quit_on_esc() {
        assert!(!parse_config("[tui]\nquit_on_esc = false").quit_on_esc);
        assert!(parse_config("[tui]\nquit_on_esc = true").quit_on_esc);
    }

    #[test]
    fn parse_config_defaults_quit_on_esc_to_true() {
        assert!(parse_config("").quit_on_esc);
        assert!(parse_config("[tui]").quit_on_esc);
        assert!(parse_config("[theme]\nmode = \"dark\"").quit_on_esc);
        assert!(parse_config("not = valid = toml").quit_on_esc);
    }

    #[test]
    fn parse_commands_reads_a_command_block() {
        let toml = r#"
            [[commands]]
            key = "e"
            command = "dev tmux edit {{filename}}"
            description = "edit in a tmux pane"
        "#;
        let cmds = parse_commands(toml);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].key, 'e');
        assert_eq!(cmds[0].command, "dev tmux edit {{filename}}");
        assert!(!cmds[0].subprocess);
        assert_eq!(cmds[0].description, "edit in a tmux pane");
    }

    #[test]
    fn parse_commands_reads_subprocess_flag() {
        let toml = r#"
            [[commands]]
            key = "E"
            command = "nvim {{filename}}"
            subprocess = true
        "#;
        let cmds = parse_commands(toml);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].subprocess);
        // Absent description defaults to empty.
        assert_eq!(cmds[0].description, "");
    }

    #[test]
    fn parse_commands_defaults_subprocess_to_false() {
        let toml = r#"
            [[commands]]
            key = "e"
            command = "echo hi"
        "#;
        let cmds = parse_commands(toml);
        assert!(!cmds[0].subprocess);
    }

    #[test]
    fn parse_commands_ignores_theme_section() {
        let toml = r##"
            [theme]
            mode = "dark"

            [[commands]]
            key = "e"
            command = "echo hi"
        "##;
        let cmds = parse_commands(toml);
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn parse_commands_empty_when_no_commands() {
        assert!(parse_commands("[theme]\nmode = \"dark\"").is_empty());
        assert!(parse_commands("").is_empty());
    }

    #[test]
    fn parse_commands_empty_on_bad_toml() {
        assert!(parse_commands("not = valid = toml").is_empty());
    }

    #[test]
    fn parse_commands_drops_multi_char_and_empty_keys() {
        let toml = r#"
            [[commands]]
            key = "ee"
            command = "echo hi"

            [[commands]]
            key = ""
            command = "echo hi"

            [[commands]]
            key = "e"
            command = "echo ok"
        "#;
        let cmds = parse_commands(toml);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "echo ok");
    }

    #[test]
    fn expand_replaces_and_shell_quotes_filename() {
        let out = expand("nvim {{filename}}", &PathBuf::from("/tmp/a.txt"));
        assert_eq!(out, "nvim '/tmp/a.txt'");
    }

    #[test]
    fn expand_keeps_path_with_space_as_one_argument() {
        let out = expand("nvim {{filename}}", &PathBuf::from("/tmp/my file.txt"));
        assert_eq!(out, "nvim '/tmp/my file.txt'");
    }

    #[test]
    fn expand_escapes_embedded_single_quote() {
        let out = expand("cat {{filename}}", &PathBuf::from("/tmp/it's.txt"));
        assert_eq!(out, "cat '/tmp/it'\\''s.txt'");
    }

    #[test]
    fn expand_passes_through_without_placeholder() {
        let out = expand("echo hi", &PathBuf::from("/tmp/a.txt"));
        assert_eq!(out, "echo hi");
    }

    #[test]
    fn expand_leaves_unknown_tokens_verbatim() {
        let out = expand("echo {{other}}", &PathBuf::from("/tmp/a.txt"));
        assert_eq!(out, "echo {{other}}");
    }
}
