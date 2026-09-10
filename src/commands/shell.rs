//! `grit shell` — the dashboard at the prompt, and the plumbing behind it.
//!
//! `init` prints a script to `eval`. The other two subcommands are what that
//! script calls back into, and the interesting one is `preview`.
//!
//! A shell cannot be handed ANSI here. The table appears *while you are still
//! typing*, which means the shell is holding it in a variable and drawing it as
//! part of the line it is editing — zsh's `POSTDISPLAY` — and escape codes in
//! there are printed as the literal characters `ESC [ 3 6 m`. What zsh colours
//! with instead is `region_highlight`: a list of character ranges over its own
//! buffer. So `preview` emits the two halves separately, plain text and the
//! ranges to paint, and the shell puts them back together.
//!
//! That is only cheap because [`crate::render::table`] keeps styling out of its
//! strings until render time. The ranges are the styled spans it was already
//! laying out, counted rather than painted.

use anyhow::Result;

use crate::cli::{
    ShellArgs, ShellCommand, ShellPathArgs, ShellPreviewArgs, ShellRefreshArgs, ShellRowsArgs,
    ShellSetupArgs,
};
use crate::context::Ctx;
use crate::render::{Theme, footer, paint, zsh_style};
use crate::shell::{self, Change, Shell};

use super::status;

pub fn run(args: &ShellArgs, ctx: &mut Ctx) -> Result<i32> {
    match &args.command {
        ShellCommand::Init(init) => {
            print!("{}", shell::script(init.shell));
            Ok(0)
        }
        ShellCommand::Enable(setup) => enable(setup, ctx),
        ShellCommand::Disable(setup) => disable(setup, ctx),
        ShellCommand::Preview(preview_args) => preview(preview_args, ctx),
        ShellCommand::Refresh(refresh_args) => refresh(refresh_args, ctx),
        ShellCommand::Rows(rows_args) => rows(rows_args, ctx),
        ShellCommand::Path(path_args) => path(path_args, ctx),
    }
}

/// Which shell to set up, and which file to write.
///
/// An unrecognised `$SHELL` is worth a sentence rather than a guess: writing
/// zsh's integration into someone's bash startup file would leave them with an
/// error on every prompt and no idea where it came from.
fn target(setup: &ShellSetupArgs) -> Result<(Shell, std::path::PathBuf)> {
    let shell = match setup.shell.or_else(Shell::from_env) {
        Some(shell) => shell,
        None => anyhow::bail!(
            "could not tell which shell you use from $SHELL\n\
             say which: grit shell enable zsh"
        ),
    };

    let path = match &setup.file {
        Some(path) => path.clone(),
        None => shell::rc_path(shell)?,
    };
    Ok((shell, path))
}

fn enable(setup: &ShellSetupArgs, ctx: &mut Ctx) -> Result<i32> {
    let (shell, path) = target(setup)?;
    let shown = crate::registry::abbreviate_home(&path);

    match shell::enable(&path, shell)? {
        Change::Added => {
            println!(
                "added the {shell} integration to {}",
                paint(&shown, Theme::path(), ctx.color)
            );
            println!(
                "  {} type `grit`, pause, and the dashboard appears under the line",
                paint("·", Theme::muted(), ctx.color)
            );
            println!(
                "  {} start a new shell to pick it up, or run: exec {shell}",
                paint("·", Theme::muted(), ctx.color)
            );
        }
        Change::AlreadyEnabled => {
            println!("the {shell} integration is already in {shown}");
        }
        Change::Managed => {
            println!("{shown} already loads the integration on a line grit did not write");
            println!("  nothing to do — it is left alone rather than duplicated");
        }
        Change::Removed | Change::NotEnabled => unreachable!("enable only adds"),
    }
    Ok(0)
}

fn disable(setup: &ShellSetupArgs, ctx: &mut Ctx) -> Result<i32> {
    let (shell, path) = target(setup)?;
    let shown = crate::registry::abbreviate_home(&path);

    match shell::disable(&path)? {
        Change::Removed => {
            println!(
                "removed the {shell} integration from {}",
                paint(&shown, Theme::path(), ctx.color)
            );
            println!(
                "  {} open shells keep it until they are restarted",
                paint("·", Theme::muted(), ctx.color)
            );
        }
        Change::NotEnabled => {
            println!("the {shell} integration is not in {shown}");
        }
        Change::Managed => {
            println!("{shown} loads the integration on a line grit did not write");
            println!("  remove that line yourself — grit will not edit what it did not add");
        }
        Change::Added | Change::AlreadyEnabled => unreachable!("disable only removes"),
    }
    Ok(0)
}

/// The cached dashboard, as the line editor needs it.
///
/// ```text
/// 3                     <- how many highlight lines follow
/// 2 5 bold              <- start, end, zsh highlight spec
/// 9 13 fg=cyan
/// 21 24 fg=yellow
///   ALIAS   BRANCH ...  <- everything after that is the table, verbatim
/// ```
///
/// Offsets are character counts from the first character of the table, so the
/// shell adds its own base — the length of what the user has typed, plus the
/// newline it puts in front of the table — and hands the result to
/// `region_highlight` unchanged.
///
/// Nothing at all is printed when there is no usable cache, or when `--max-age`
/// says the one there is too old to put in front of someone. The caller has
/// just started a refresh and will ask again in a second; a placeholder row
/// would only flicker, and a stale table read as a live one is the failure this
/// whole feature has to avoid.
fn preview(args: &ShellPreviewArgs, ctx: &mut Ctx) -> Result<i32> {
    if args.max_rows == Some(0) {
        return Ok(0);
    }

    let repos = ctx.registry.all();
    let Some(cache) = status::read_cache(ctx) else {
        return Ok(0);
    };
    if args.max_age.is_some_and(|max| cache.age_seconds() > max) {
        return Ok(0);
    }
    let rows = status::select_cached(&cache, &repos);
    if rows.is_empty() {
        return Ok(0);
    }

    let shown = match args.max_rows {
        Some(max) if rows.len() > max => &rows[..max],
        _ => &rows[..],
    };

    // The summary counts every repo, not just the ones that fit.
    let mut parts = status::summarise(&rows);
    if shown.len() < rows.len() {
        parts.push(format!("{} more", rows.len() - shown.len()));
    }
    parts.push(format!("{} ago", cache.age()));

    let (mut text, mut ranges) = status::build_table(shown, ctx).render_highlighted();

    // The blank line and footer `grit status` prints under its table. The
    // footer goes through the same helper so the two cannot drift apart, and
    // its highlight covers the leading indent too — colouring a space is
    // invisible, and it saves this from having to know how wide the indent is.
    text.push('\n');
    let line = footer(&parts, false);
    let line = line.trim_end_matches('\n');
    let start = text.chars().count();
    let end = start + line.chars().count();
    if end > start {
        ranges.push(crate::render::Highlight {
            start,
            end,
            style: Theme::muted(),
        });
    }
    text.push_str(line);
    text.push('\n');

    let specs: Vec<String> = ranges
        .iter()
        .filter_map(|h| zsh_style(h.style).map(|spec| format!("{} {} {}", h.start, h.end, spec)))
        .collect();

    println!("{}", specs.len());
    for spec in &specs {
        println!("{spec}");
    }
    print!("{text}");

    Ok(0)
}

/// Where an alias points, unadorned, for a shell to `cd` into.
///
/// `grit show --json` already carries this, but reading it from a key binding
/// means a JSON parser in the shell — and the two answers that were tried,
/// grep-and-sed and a jq dependency, are respectively wrong on a path with a
/// quote in it and not installed.
///
/// The path is printed raw: not abbreviated to `~`, which is for reading, and
/// with no trailing newline of consequence, so `cd -- "$(grit shell path api)"`
/// is the whole of the caller's side.
fn path(args: &ShellPathArgs, ctx: &mut Ctx) -> Result<i32> {
    let repo = ctx.registry.get(&args.alias)?;
    println!("{}", repo.path().display());
    Ok(0)
}

/// The dashboard's rows alone, one repo per line.
///
/// What the picker reads on stdin. No header, no rule and no footer, because
/// every line a picker is given is a line it will let you select — and no
/// margin, because the alias has to be the row's first field for a preview
/// command to name the repo with `{1}`.
///
/// Painted with ANSI rather than handed over as text plus ranges: the picker
/// is a separate program drawing on the terminal itself, not the line editor
/// holding a string, so this is the one place where escapes are what is
/// wanted. `--color` still decides, so a pipe into a file is plain.
fn rows(args: &ShellRowsArgs, ctx: &mut Ctx) -> Result<i32> {
    let repos = ctx.registry.all();
    if repos.is_empty() {
        return Ok(0);
    }

    // The cache is what makes the picker open instantly; falling back to a
    // live reading when there is not one — and leaving it behind for next
    // time — is what stops the very first press showing an empty list.
    let cached = args
        .cached
        .then(|| status::read_cache(ctx))
        .flatten()
        .map(|cache| status::select_cached(&cache, &repos));

    let rows = match cached {
        Some(rows) => rows,
        None => {
            let rows = status::collect(repos);
            let _ = status::try_write_cache(ctx, &rows);
            rows
        }
    };

    print!(
        "{}",
        status::build_table(&rows, ctx)
            .no_header()
            .no_rule()
            .no_indent()
            .render()
    );
    Ok(0)
}

/// Take a reading into the cache and print nothing.
///
/// Errors reaching a repository are not errors here — they are cached as the
/// row they will be rendered as. Only failing to *write* the cache is worth
/// reporting, and it goes to stderr, which the integration discards.
fn refresh(args: &ShellRefreshArgs, ctx: &mut Ctx) -> Result<i32> {
    // `let`-chains would read better here and need a newer rustc than the
    // `rust-version` in Cargo.toml promises.
    let still_fresh = args.max_age > 0
        && status::read_cache(ctx).is_some_and(|cache| cache.age_seconds() < args.max_age);
    if still_fresh {
        return Ok(0);
    }

    let repos = ctx.registry.all();
    if repos.is_empty() {
        return Ok(0);
    }

    status::try_write_cache(ctx, &status::collect(repos))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser as _;

    /// The hidden subcommands are still real ones, reachable the ordinary way.
    #[test]
    fn the_hidden_subcommands_parse() {
        let cli = Cli::try_parse_from(["grit", "shell", "preview", "--max-rows", "8"]).unwrap();
        let Some(Command::Shell(args)) = cli.command else {
            panic!("expected shell");
        };
        let ShellCommand::Preview(preview) = args.command else {
            panic!("expected preview");
        };
        assert_eq!(preview.max_rows, Some(8));
    }

    #[test]
    fn refresh_defaults_to_taking_a_reading_unconditionally() {
        let cli = Cli::try_parse_from(["grit", "shell", "refresh"]).unwrap();
        let Some(Command::Shell(args)) = cli.command else {
            panic!("expected shell");
        };
        let ShellCommand::Refresh(refresh) = args.command else {
            panic!("expected refresh");
        };
        assert_eq!(refresh.max_age, 0);
    }
}
