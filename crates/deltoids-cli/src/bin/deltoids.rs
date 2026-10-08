//! `deltoids` — single CLI for the deltoids toolkit.
//!
//! Subcommands:
//!
//! - `pager`     ANSI diff filter for `less` / `core.pager`
//! - `tui`       scrolling TUI over the working-tree diff
//!
//! Default (no subcommand): if stdin is a pipe, run `pager` (so
//! `git config core.pager 'deltoids | less -R'` keeps working). On a
//! TTY, open the `tui`.

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use deltoids_cli::cli::{pager, tui};

#[derive(Debug, Parser)]
#[command(
    name = "deltoids",
    version,
    disable_version_flag = true,
    about = "Diff renderer and scrolling TUI.",
    long_about = "\
The deltoids toolkit. Run `deltoids <subcommand> --help` for details. \
With no subcommand, a piped diff runs the pager (preserving \
`git config core.pager 'deltoids | less -R'`) and a TTY opens the TUI."
)]
struct Cli {
    /// Print version and exit.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// ANSI diff filter for less / core.pager.
    Pager(pager::Args),
    /// Scrolling TUI over the working-tree diff.
    Tui(tui::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Pager(args)) => pager::run(args),
        Some(Command::Tui(args)) => tui::run(args),
        None => {
            // Smart default: a piped diff feeds the pager (preserving
            // `core.pager`); a TTY opens the unified TUI.
            if std::io::stdin().is_terminal() {
                tui::run(tui::Args::default())
            } else {
                pager::run(pager::Args::default())
            }
        }
    }
}
