//! Shared terminal session guard for TUI subcommands.

use std::io::{self, Write};

use ratatui::Terminal;

pub struct TerminalSession;

impl TerminalSession {
    pub fn enter<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::event::EnableMouseCapture
        )
        .map_err(|err| format!("failed to enter screen: {err}"))?;
        terminal
            .clear()
            .map_err(|err| format!("failed to clear screen: {err}"))?;
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
