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
panel forward (Files -> Traces) and [ to cycle back.

Keys:
- [ / ]:           cycle Files / Traces mode
- Tab / 1 / 2:     focus panes in the current mode
- j / k / arrows:  move within the focused pane (between diff lines in
                   the Traces diff pane)
- Shift+J / K:     scroll the diff pane
- PgUp / PgDn:     page the focused pane
- < / >:           narrow / widen the sidebar (or drag the divider)
- ?:               toggle the help popup
- q:               quit

Review comments, with the Traces diff pane focused (3):
- c:               comment on the diff line under the cursor
- d:               delete that line's comment
- y:               copy every comment on the trace as one prompt

Comments live in the running session only; they are never written to disk.
Copying builds a prompt listing each file, line, diff line, and note, ready
to paste into a coding agent.

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
