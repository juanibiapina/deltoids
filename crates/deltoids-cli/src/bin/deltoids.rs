//! `deltoids` — single CLI for the deltoids toolkit.
//!
//! Subcommands:
//!
//! - `pager`   ANSI diff filter for `less` / `core.pager`
//! - `review`  scrolling TUI viewer for a unified diff
//! - `edit`    agent edit tool, appends to a trace
//! - `write`   agent write tool, appends to a trace
//! - `traces`  browse edit/write traces for the current dir
//! - `hook`    coding-agent lifecycle adapters (Claude Code, …)
//!
//! Default (no subcommand): if stdin is a pipe, run `pager` (so
//! `git config core.pager 'deltoids | less -R'` keeps working). If
//! stdin is a TTY, print top-level help.

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

use deltoids_cli::cli::{edit, hook, pager, review, traces, write};

#[derive(Debug, Parser)]
#[command(
    name = "deltoids",
    about = "Diff renderer, scrolling TUI, agent edit tools, and trace browser.",
    long_about = "\
The deltoids toolkit. Run `deltoids <subcommand> --help` for details on \
each subcommand. With no subcommand and a unified diff piped in, \
`deltoids` runs the pager (preserving `git config core.pager 'deltoids | less -R'`)."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// ANSI diff filter for less / core.pager.
    Pager(pager::Args),
    /// Scrolling TUI viewer for a unified diff.
    Review(review::Args),
    /// Agent edit tool — appends to a trace.
    Edit(edit::Args),
    /// Agent write tool — appends to a trace.
    Write(write::Args),
    /// Browse edit/write traces for the current directory.
    Traces(traces::Args),
    /// Coding-agent lifecycle adapters (e.g. Claude Code PostToolUse).
    #[command(hide = true)]
    Hook(hook::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Pager(args)) => pager::run(args),
        Some(Command::Review(args)) => review::run(args),
        Some(Command::Edit(args)) => edit::run(args),
        Some(Command::Write(args)) => write::run(args),
        Some(Command::Traces(args)) => traces::run(args),
        Some(Command::Hook(args)) => hook::run(args),
        None => {
            // No subcommand: pipe → pager (preserve `core.pager` use),
            // TTY → help.
            if std::io::stdin().is_terminal() {
                let mut cmd = Cli::command();
                let _ = cmd.print_help();
                println!();
                ExitCode::SUCCESS
            } else {
                pager::run(pager::Args::default())
            }
        }
    }
}
