//! Entry point. Parse, dispatch, report, exit.
//!
//! The only logic here is turning an error into a message and an exit code —
//! everything else lives in the library so it can be tested.

use std::process::ExitCode;

use clap::Parser as _;

use grit::cli::{Cli, normalize_args};
use grit::commands;
use grit::context::Ctx;
use grit::render::{Theme, paint};

fn main() -> ExitCode {
    // clap handles --help/--version itself, and exits with 2 on a usage error.
    let cli = Cli::parse_from(normalize_args(std::env::args_os()));

    match real_main(&cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            report(&err, cli.color);
            ExitCode::FAILURE
        }
    }
}

fn real_main(cli: &Cli) -> anyhow::Result<i32> {
    let mut ctx = Ctx::new(cli.color)?;
    commands::dispatch(cli, &mut ctx)
}

/// Print an error to stderr.
///
/// grit's errors carry their own hints on trailing lines ("run `grit show` to
/// see what is registered"), so the first line is the error proper and the rest
/// is dimmed underneath it.
fn report(err: &anyhow::Error, color: grit::render::ColorChoice) {
    use std::io::IsTerminal as _;
    let color = color.enabled_for(std::io::stderr().is_terminal());

    let text = err.to_string();
    let mut lines = text.lines();

    eprintln!(
        "{} {}",
        paint("error:", Theme::warn(), color),
        lines.next().unwrap_or("something went wrong"),
    );
    for line in lines {
        eprintln!("       {}", paint(line, Theme::muted(), color));
    }

    // Context attached with `.with_context(...)` arrives as a cause.
    for cause in err.chain().skip(1) {
        eprintln!(
            "       {}",
            paint(&format!("caused by: {cause}"), Theme::muted(), color)
        );
    }
}
