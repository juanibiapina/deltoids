//! Run helpers for custom commands: the background (no-terminal) path and
//! the foreground (suspend/restore) path.
//!
//! Not unit-tested: correctness here is the terminal sequencing
//! (foreground) and the stdio-null discipline (background), verified by
//! manual acceptance. The pure logic (parsing, expansion, routing) lives
//! in [`super::command`] and the shell.

use std::io::{self, Stdout, Write};
use std::process::{Command, Stdio};

use crossterm::cursor;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

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

/// Suspend the TUI, hand the terminal to `command` (inherited stdio), then
/// restore and force a full repaint.
///
/// The repaint recreates the `Terminal` rather than calling
/// `Terminal::clear()`: on this ratatui version `clear()` issues an
/// `ESC[6n` cursor query that busy-spins forever on macOS. A fresh
/// `Terminal` has empty buffers, so the next `draw` repaints every cell,
/// identical to the startup path.
pub(super) fn run_foreground(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    command: &str,
) -> Result<(), String> {
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
    *terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|err| format!("failed to restore screen: {err}"))?;
    status.map_err(|err| format!("failed to run command: {err}"))?;
    Ok(())
}

/// Leave the TUI: same teardown as `TerminalSession::drop`, in place.
fn leave_tui() -> Result<(), String> {
    disable_raw_mode().map_err(|err| format!("failed to disable raw mode: {err}"))?;
    execute!(
        io::stdout(),
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
        EnableMouseCapture
    )
    .map_err(|err| format!("failed to re-enter screen: {err}"))?;
    Ok(())
}
