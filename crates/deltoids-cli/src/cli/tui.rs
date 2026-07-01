//! `deltoids tui`: the unified scrolling TUI.
//!
//! Interactive (TTY stdout): opens on the working-tree diff (Files mode)
//! when there are local changes, otherwise on the trace browser (Traces
//! mode); press `[` / `]` to toggle. Headless (non-TTY stdout): renders
//! the Traces scripted snapshot from stdin keys, used by tests and
//! non-interactive callers.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::cli::browse;

const OVERVIEW: &str = r#"Unified scrolling TUI.

Opens on the working-tree diff (Files mode) when you have local changes,
otherwise on the trace browser (Traces mode). Press ] to cycle the left
panel forward (Files -> Traces -> Live) and [ to cycle back. Live is an
ephemeral, in-memory feed of working-tree edits as they happen.

Keys:
- [ / ]:           cycle Files / Traces / Live mode
- Tab / 1 / 2 / 3: focus panes in the current mode
- j / k / arrows:  move within the focused pane
- PgUp / PgDn:     scroll the diff pane
- < / >:           narrow / widen the sidebar (or drag the divider)
- ?:               toggle the help popup
- q / Esc:         quit

Set RV_NO_ICONS=1 to disable nerd-font glyphs in the sidebar.
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {}

pub fn run(_args: Args) -> ExitCode {
    match run_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("deltoids: {err}");
            ExitCode::from(1)
        }
    }
}

fn run_inner() -> Result<(), String> {
    if io::stdout().is_terminal() {
        browse::run(browse::smart_initial_mode())
    } else {
        browse::run_traces_scripted()
    }
}
