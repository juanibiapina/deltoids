//! Crate root for `deltoids-cli`: the `pager` and `tui` subcommands.

pub mod cli;
mod judgments;
mod lsp;
mod sidebar;
#[cfg(test)]
mod test_git;
