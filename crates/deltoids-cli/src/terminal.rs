//! Shared terminal session guard for TUI subcommands.

use std::io::{self, Write};

pub struct TerminalSession;

impl TerminalSession {
    // Do NOT call `terminal.clear()` here. On `ratatui-core >= 0.1.1`,
    // `Terminal::clear()` snapshots the cursor with an `ESC[6n` query via
    // `get_cursor_position`. Routed through ratatui's crossterm backend
    // without `use-dev-tty`, that read busy-spins forever on macOS, hanging
    // before the first draw. The alternate screen is already blank after
    // `?1049h`, so the first `terminal.draw` paints everything.
    pub fn enter() -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::event::EnableMouseCapture
        )
        .map_err(|err| format!("failed to enter screen: {err}"))?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = io::stdout().flush();
    }
}
