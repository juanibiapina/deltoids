//! `deltoids traces` — browse edit/write traces for the current dir.

use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::ErrorResponse;
use crate::tui;

const OVERVIEW: &str = r#"TUI for browsing edit/write traces for the current directory.

Keys:
- Tab:          switch focus between the traces pane and the entries pane
- j / k / arrows: move within the focused pane
- PgUp / PgDn:  scroll the diff pane
- < / >:        narrow / widen the sidebar (or drag the divider with the mouse)
- q / Esc:      quit
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {}

pub fn run(_args: Args) -> ExitCode {
    match tui::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let response = ErrorResponse {
                ok: false,
                error,
                trace_id: None,
                message: None,
            };
            eprintln!(
                "{}",
                serde_json::to_string(&response).expect("error response should serialize")
            );
            ExitCode::from(1)
        }
    }
}
