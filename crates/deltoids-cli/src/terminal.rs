//! Shared terminal session guard for TUI subcommands.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use signal_hook::SigId;
use signal_hook::consts::{SIGHUP, SIGQUIT, SIGTERM};

const QUIT_SIGNALS: [i32; 3] = [SIGTERM, SIGHUP, SIGQUIT];

pub struct TerminalSession {
    quit: Arc<AtomicBool>,
    signal_ids: Vec<SigId>,
}

impl TerminalSession {
    // Do NOT call `terminal.clear()` here. On `ratatui-core >= 0.1.1`,
    // `Terminal::clear()` snapshots the cursor with an `ESC[6n` query via
    // `get_cursor_position`. Routed through ratatui's crossterm backend
    // without `use-dev-tty`, that read busy-spins forever on macOS, hanging
    // before the first draw. The alternate screen is already blank after
    // `?1049h`, so the first `terminal.draw` paints everything.
    pub fn enter() -> Result<Self, String> {
        let quit = Arc::new(AtomicBool::new(false));
        let signal_ids = QUIT_SIGNALS
            .iter()
            .map(|&signal| signal_hook::flag::register(signal, Arc::clone(&quit)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to register signal handler: {err}"))?;
        let session = Self { quit, signal_ids };
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::event::EnableMouseCapture,
            crossterm::event::EnableFocusChange
        )
        .map_err(|err| format!("failed to enter screen: {err}"))?;
        Ok(session)
    }

    pub fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::Relaxed)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        for id in self.signal_ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableFocusChange,
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = io::stdout().flush();
    }
}
