//! `deltoids tui`: the scrolling TUI over the working-tree diff. Needs a
//! terminal on stdout.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::cli::browse;

const OVERVIEW: &str = r#"Scrolling TUI over the working-tree diff and the repository history,
laid out like lazygit: the Status panel on the left, and the main view on
the right showing what the Status panel selects.

The Status panel reads like `but status`: the uncommitted changes on top,
then the checked-out branch with its commits since the common base (the
merge base with origin/HEAD, origin/main, main, or its upstream), then the
other local branches. On a file, main shows its diff; on a branch, its
log; on a commit, its diff.

Keys:
- j / k:           move within the side panel, across changes and
                   branches (between diff lines in the main view)
- Tab / Shift+Tab: jump between the changes and the branches
- Enter:           on a file, focus its diff in the main view; on a
                   commit, review it; on the stacked branch, review all
                   its commits; on another branch, list its commits
- Esc:             leave the main view, a commit list, or a review
- Tab (main view): switch the staged / unstaged diff
- Shift+J / K:     scroll the diff pane
- PgUp / PgDn:     page the focused pane
- < / >:           narrow / widen the sidebar (or drag the divider)
- ?:               toggle the help popup
- q:               quit

The diff shows staged and unstaged changes separately. When the selection
has both, the diff title reads "Staged - Unstaged" and s (or Tab in the main
view) switches between them.

In a GitButler workspace, the Status panel follows GitButler: it shows every
uncommitted change in one column (GitButler has no staging, so Space and a
change nothing), then each applied stack and the unapplied branches, and
discard runs `but discard`, so `but undo` brings changes back. Any other
repository works as plain git.

Review comments, with a diff in the main view focused (0):
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
