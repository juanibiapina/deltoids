//! `deltoids pr`: review one GitHub pull request in the TUI. Needs a
//! terminal on stdout.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::cli::browse;
use crate::pull_request::PrRef;

const OVERVIEW: &str = r#"Opens one GitHub pull request as a read-only review: its changed files on
the left and their diff on the right, from the merge base to the pull
request's head, as GitHub's "Files changed" tab shows it. The top row of
the file tree shows the pull request itself: its title, author, branches,
link, and description, then its commits.

Where the commits come from:
- Run it inside a clone of the pull request's repository (one of its
  remotes points at owner/repo) and deltoids fetches the commits it lacks
  into that clone. It changes no branch, ref, or file there.
- Anywhere else, it fetches the two commits, without history or unchanged
  files, into a cache repository under $XDG_CACHE_HOME/deltoids
  (~/.cache/deltoids by default).

Needs the GitHub CLI, gh, installed and logged in (gh auth login).

From another program: run it in the foreground on the same terminal, with
the clone as the working directory when you know one. It exits 0 when the
user quits with q. Errors after startup show in the TUI.

Keys:
- j / k:       move through the files (between diff lines in the diff)
- Enter:       focus the selected file's diff
- Esc:         leave the diff
- c / d / y:   comment on a diff line, delete it, copy every comment
- ?:           all keys
- q:           quit

Comments last until you quit. Copy them with y before quitting."#;

#[derive(Debug, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    /// The pull request: owner/repo#123 or its URL.
    #[arg(value_name = "PR")]
    pr: PrRef,
}

pub fn run(args: Args) -> ExitCode {
    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("deltoids: {err}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(args: Args) -> Result<(), String> {
    if !io::stdout().is_terminal() {
        return Err("pr: needs a terminal".to_string());
    }
    let cwd = std::env::current_dir().map_err(|err| format!("pr: {err}"))?;
    browse::run_pull_request(args.pr, cwd)
}
