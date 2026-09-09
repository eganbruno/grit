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
pub mod shell;
pub mod show;
pub mod status;

use std::ffi::OsString;

use anyhow::{Result, bail};
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
        Some(Command::Shell(args)) => shell::run(args, ctx),
        Some(Command::External(argv)) => match help_or_version(argv)? {
            Some(code) => Ok(code),
            None => run::run(argv, cli.keep_going, ctx),
        },
        None => {
            Cli::command().print_help()?;
            Ok(0)
        }
    }
}

/// `grit help` and `grit version`, which clap cannot own here.
///
/// `disable_help_subcommand` stops clap listing a `help` command beside the
/// passthrough forms, and `--version` is a flag rather than a subcommand — so
/// both bare words reached the external subcommand and came back as "no repo
/// registered under alias `help`". Every tool a user arrives from takes the
/// bare word, and both names are already in `RESERVED_ALIASES`, so answering
/// them here cannot shadow anybody's alias.
///
/// Returns `None` when the first word is neither, leaving the passthrough to
/// deal with it.
fn help_or_version(argv: &[OsString]) -> Result<Option<i32>> {
    let Some(word) = argv.first().and_then(|arg| arg.to_str()) else {
        return Ok(None);
    };

    match word {
        "version" => {
            // clap's own rendering, so `grit version` and `grit --version`
            // cannot drift apart.
            print!("{}", Cli::command().render_version());
            Ok(Some(0))
        }
        "help" => {
            let mut cmd = Cli::command();
            // `build` is what fills in each subcommand's display name. Without
            // it a lifted-out subcommand renders `Usage: status …` rather than
            // `Usage: grit status …`.
            cmd.build();

            // Walk the words, so `grit help shell enable` works as well as
            // `grit help status` does.
            for name in argv[1..].iter().filter_map(|arg| arg.to_str()) {
                let Some(sub) = cmd.find_subcommand(name).cloned() else {
                    bail!(
                        "`{name}` is not a grit command\n\
                         run `grit help` for the list, or `grit show` if you meant an alias"
                    );
                };
                cmd = sub;
            }

            // `grit help | head` closes the pipe on us, and that is the reader
            // being done rather than a failure to report.
            if let Err(e) = cmd.print_long_help() {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(e.into());
                }
            }
            Ok(Some(0))
        }
        _ => Ok(None),
    }
}

/// Pretty-printed JSON, used by every `--json` flag so they agree on shape.
pub fn to_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}
