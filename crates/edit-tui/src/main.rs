mod highlight;
mod tui;

use std::process::ExitCode;

use clap::Parser;
use edit::ErrorResponse;

const OVERVIEW: &str = r#"TUI for browsing edit/write traces for the current directory.

Keys:
- Tab:          switch focus between the traces pane and the entries pane
- j / k / arrows: move within the focused pane
- PgUp / PgDn:  scroll the diff pane
- q / Esc:      quit
"#;

#[derive(Debug, Parser)]
#[command(
    name = "edit-tui",
    about = "TUI for reviewing edit/write traces.",
    after_help = OVERVIEW
)]
struct Cli {}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string(&error).expect("error response should serialize")
            );
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), ErrorResponse> {
    let _ = Cli::parse();
    tui::run().map_err(|error| ErrorResponse {
        ok: false,
        error,
        trace_id: None,
        message: None,
    })
}
