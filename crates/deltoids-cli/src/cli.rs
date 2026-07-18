//! Subcommand implementations for the `deltoids` binary. Each
//! submodule exposes an `Args` struct (clap `Args` + `Parser` derive)
//! and a `run(args: Args) -> ExitCode` function that the top-level
//! dispatcher in `bin/deltoids.rs` invokes.

pub mod browse;
pub mod edit;
pub mod hash_edit;
pub mod hash_read;
pub mod hook;
pub mod pager;
pub mod serve;
pub mod tui;
pub mod write;
