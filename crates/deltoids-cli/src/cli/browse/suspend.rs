//! Run helpers for custom commands: the background (no-terminal) path and
//! the foreground (suspend/restore) path.
//!
//! The focus CPU probe verifies foreground terminal sequencing and the
//! restored screen. Parsing, expansion, and routing live in
//! [`super::command`] and the shell.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use super::command::shell_quote;
use super::terminal::{pop_key_flags, push_key_flags};
use super::{BrowseTerminal, buffered_backend};
use crossterm::cursor;
use crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;

/// Run `command` in the background without touching the terminal.
///
/// stdin/stdout/stderr are set to null so the child can never scribble on
/// our alt screen (that would corrupt it / cause flicker). Blocks the
/// loop until the child exits, which is fine for dispatch-and-return
/// commands. The exit code is ignored; only a spawn failure is returned
/// (and the caller ignores that too in v1).
pub(super) fn run_background(command: &str) -> Result<(), String> {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("failed to run command: {err}"))?;
    Ok(())
}

/// The shell line that opens `path` in `$EDITOR`. It runs through the
/// shell, so `EDITOR="code --wait"` works. With `$EDITOR` unset or
/// blank, an error: no other variable or fallback editor is read.
pub(super) fn editor_command(path: &Path) -> Result<String, String> {
    let editor = std::env::var("EDITOR").unwrap_or_default();
    if editor.trim().is_empty() {
        return Err("$EDITOR is not set: set it to open the message in an editor".to_string());
    }
    Ok(format!("{editor} {}", shell_quote(&path.to_string_lossy())))
}

/// Run an [`editor_command`] in the foreground. A non-zero exit is an
/// error.
pub(super) fn edit(terminal: &mut BrowseTerminal, command: &str) -> Result<(), String> {
    if run_foreground(terminal, command)?.success() {
        Ok(())
    } else {
        Err("The editor exited with an error".to_string())
    }
}

/// Suspend the TUI, hand the terminal to `command` (inherited stdio), then
/// restore and force a full repaint. Returns the child's exit status.
///
/// The repaint recreates the `Terminal` rather than calling
/// `Terminal::clear()`: on this ratatui version `clear()` issues an
/// `ESC[6n` cursor query that busy-spins forever on macOS. A fresh
/// `Terminal` has empty buffers, so the next `draw` repaints every cell,
/// identical to the startup path.
pub(super) fn run_foreground(
    terminal: &mut BrowseTerminal,
    command: &str,
) -> Result<ExitStatus, String> {
    leave_tui()?;
    let status = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    enter_tui()?;
    // Recreate the terminal so the next draw repaints every cell without
    // the cursor-query hang of `Terminal::clear()`.
    *terminal = Terminal::new(buffered_backend())
        .map_err(|err| format!("failed to restore screen: {err}"))?;
    status.map_err(|err| format!("failed to run command: {err}"))
}

/// Leave the TUI: same teardown as `TerminalSession::drop`, in place.
fn leave_tui() -> Result<(), String> {
    pop_key_flags();
    disable_raw_mode().map_err(|err| format!("failed to disable raw mode: {err}"))?;
    execute!(
        io::stdout(),
        DisableFocusChange,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    )
    .map_err(|err| format!("failed to leave screen: {err}"))?;
    io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush: {err}"))?;
    Ok(())
}

/// Re-enter the TUI: same setup as `TerminalSession::enter`.
fn enter_tui() -> Result<(), String> {
    enable_raw_mode().map_err(|err| format!("failed to enable raw mode: {err}"))?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        cursor::Hide,
        EnableMouseCapture,
        EnableFocusChange
    )
    .map_err(|err| format!("failed to re-enter screen: {err}"))?;
    push_key_flags();
    Ok(())
}
