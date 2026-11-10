//! Crate root for `deltoids-cli`: the `pager`, `tui`, and `pr` subcommands.

pub mod cli;
mod jev;
mod lsp;
mod pull_request;
mod sidebar;
#[cfg(test)]
mod test_git;
