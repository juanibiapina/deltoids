//! `deltoids` — single CLI for the deltoids toolkit.
//!
//! Subcommands:
//!
//! - `pager`     ANSI diff filter for `less` / `core.pager`
//! - `tui`       unified scrolling TUI (working-tree diff + trace browser)
//! - `serve`     read-only HTTP server + web app for reviewing traces
//! - `edit`      agent edit tool, appends to a trace
//! - `write`     agent write tool, appends to a trace
//! - `hashread`  agent read tool that emits hashline anchors
//! - `hashedit`  agent edit tool using hashline anchors
//! - `hook`      coding-agent lifecycle adapters (Claude Code, …)
//!
//! Default (no subcommand): if stdin is a pipe, run `pager` (so
//! `git config core.pager 'deltoids | less -R'` keeps working). On a
//! TTY, open the `tui`.

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use deltoids_cli::cli::{edit, hash_edit, hash_read, hook, pager, serve, tui, write};

#[derive(Debug, Parser)]
#[command(
    name = "deltoids",
    version,
    disable_version_flag = true,
    about = "Diff renderer, unified scrolling TUI, and agent edit tools.",
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
    /// Unified scrolling TUI: working-tree diff and trace browser.
    Tui(tui::Args),
    /// Serve traces over HTTP for the mobile web reviewer.
    Serve(serve::Args),
    /// Agent edit tool — appends to a trace.
    Edit(edit::Args),
    /// Agent write tool — appends to a trace.
    Write(write::Args),
    /// Agent read tool that emits hashline anchors.
    Hashread(hash_read::Args),
    /// Agent edit tool using hashline anchors.
    Hashedit(hash_edit::Args),
    /// Coding-agent lifecycle adapters (e.g. Claude Code PostToolUse).
    #[command(hide = true)]
    Hook(hook::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Pager(args)) => pager::run(args),
        Some(Command::Tui(args)) => tui::run(args),
        Some(Command::Serve(args)) => serve::run(args),
        Some(Command::Edit(args)) => edit::run(args),
        Some(Command::Write(args)) => write::run(args),
        Some(Command::Hashread(args)) => hash_read::run(args),
        Some(Command::Hashedit(args)) => hash_edit::run(args),
        Some(Command::Hook(args)) => hook::run(args),
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
