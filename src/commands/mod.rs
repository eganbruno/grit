//! Command implementations, and the one place they are wired up.
//!
//! # Adding a command
//!
//! 1. add a variant to [`crate::cli::Command`]
//! 2. add `commands/<name>.rs` with `pub fn run(args, ctx) -> Result<...>`
//! 3. add the match arm below — the compiler will insist on it
//! 4. unit-test the pure parts inline, and add `tests/cli_<name>.rs`
//!
//! Commands go through [`crate::registry::Registry`] and [`crate::vcs::Vcs`]
//! rather than touching the filesystem or spawning processes directly. That is
//! what keeps them testable, and what will let a new VCS backend work without
//! any change here.

pub mod register;
pub mod run;
pub mod show;
pub mod status;

use anyhow::Result;
use clap::CommandFactory as _;
use serde::Serialize;

use crate::cli::{Cli, Command};
use crate::context::Ctx;

/// Run the selected command and return the process exit code.
pub fn dispatch(cli: &Cli, ctx: &mut Ctx) -> Result<i32> {
    match &cli.command {
        Some(Command::Register(args)) => register::register(args, ctx).map(|_| 0),
        Some(Command::Remove(args)) => register::remove(args, ctx).map(|_| 0),
        Some(Command::Show(args)) => show::run(args, ctx).map(|_| 0),
        Some(Command::Status(args)) => status::run(args, ctx),
        Some(Command::External(argv)) => run::run(argv, cli.keep_going, ctx),
        None => {
            Cli::command().print_help()?;
            Ok(0)
        }
    }
}

/// Pretty-printed JSON, used by every `--json` flag so they agree on shape.
pub fn to_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}
