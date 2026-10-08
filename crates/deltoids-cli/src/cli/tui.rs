//! `deltoids tui`: the scrolling TUI over the working-tree diff. Needs a
//! terminal on stdout.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::cli::browse;

const OVERVIEW: &str = r#"Scrolling TUI over the working-tree diff.

Keys:
- Tab / 1 / 2:     focus the sidebar / diff pane
- j / k / arrows:  move within the focused pane (between diff lines in
                   the diff pane)
- Shift+J / K:     scroll the diff pane
- PgUp / PgDn:     page the focused pane
- < / >:           narrow / widen the sidebar (or drag the divider)
- ?:               toggle the help popup
- q:               quit

The diff shows staged and unstaged changes separately. When the selection
has both, the diff title reads "Staged - Unstaged" and s switches between them.

Review comments, with the diff pane focused (2):
- c:               comment on the diff line under the cursor
- d:               delete that line's comment
- y:               copy every comment in the view

Comments live in the running session only; they are never written to disk.
Copying gives one block per comment: the file and line, the quoted diff
line, and the note, ready to paste into a coding agent. Comments follow
their line as the working tree changes, and are marked outdated when the
line moves on.

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
    if !io::stdout().is_terminal() {
        return Err("tui: needs a terminal".to_string());
    }
    browse::run()
}
